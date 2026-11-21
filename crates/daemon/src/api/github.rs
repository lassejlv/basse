//! Routes GitHub and the operator's browser call without a bearer token: the manifest flow
//! pages, the install callback and webhooks. Each checks its own proof.

use super::Query;
use super::sources::{hash_state, record};
use crate::deploy::{self, Plan};
use crate::error::{ApiResult, AppError, bad_request};
use crate::github::{self, Github, InstallationEvent, PushEvent};
use crate::sources;
use crate::state::AppState;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{Html, IntoResponse, Redirect, Response};
use chrono::Utc;
use common::*;
use serde::Deserialize;
use std::sync::Arc;
use subtle::ConstantTimeEq as _;

#[derive(Deserialize)]
pub struct StateQuery {
    state: String,
}

/// Escapes text for HTML element content and quoted attribute values.
fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn page(title: &str, body: &str) -> Html<String> {
    Html(format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width\"><title>{title}</title>\
         <style>body{{font:15px/1.5 ui-monospace,monospace;background:#0a0a0a;color:#ededed;max-width:40rem;margin:4rem auto;padding:0 1rem}}p{{color:#a1a1a1}}button{{font:inherit;padding:.5rem 1rem}}</style>\
         </head><body><h1>{title}</h1>{body}</body></html>",
        title = escape(title)
    ))
}

/// The setup state must match the one stored for this source and not have expired.
async fn check_state(state: &AppState, id: &str, presented: &str) -> ApiResult<()> {
    let stored = state.db.source_setup_state(id).await?;
    let ok = stored.is_some_and(|stored| {
        bool::from(hash_state(presented).as_bytes().ct_eq(stored.as_bytes()))
    });
    if !ok {
        bad_request!(
            "this setup link expired or was already used: start again with `{} source github create`",
            brand::CLI_BIN
        );
    }
    Ok(())
}

/// An HTML page that posts the app manifest to GitHub's new-app form.
pub async fn start(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(q): Query<StateQuery>,
) -> ApiResult<Response> {
    check_state(&state, &id, &q.state).await?;
    let rec = record(&state, &id).await?;
    let instance = state
        .instance_url()
        .ok_or_else(|| AppError::BadRequest("the instance domain is no longer verified".into()))?;
    let domain = instance.trim_start_matches("https://");
    let manifest = github::manifest(&github::app_name(domain), &instance, &id);
    let action = github::new_app_url(&rec.source.html_url, rec.organization.as_deref(), &q.state);
    let body = format!(
        "<p>Sending you to GitHub to create the app for source {name}.</p>\
         <form method=\"post\" action=\"{action}\"><input type=\"hidden\" name=\"manifest\" value=\"{manifest}\">\
         <button type=\"submit\">Create the GitHub App</button></form>\
         <script>document.forms[0].submit()</script>",
        name = escape(&rec.source.name),
        action = escape(&action),
        manifest = escape(&manifest.to_string()),
    );
    Ok(page("Create the GitHub App", &body).into_response())
}

#[derive(Deserialize)]
pub struct CallbackQuery {
    code: String,
    state: String,
}

/// GitHub redirects here after the app was created: trade the code for the app's credentials,
/// then send the browser on to install the app.
pub async fn callback(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(q): Query<CallbackQuery>,
) -> ApiResult<Response> {
    check_state(&state, &id, &q.state).await?;
    let mut rec = record(&state, &id).await?;
    let app = Github::new(&rec.source.api_url)?
        .convert_manifest(&q.code)
        .await?;
    let value = SecretValue::GithubApp {
        app_id: app.id,
        private_key: app.pem,
        webhook_secret: app.webhook_secret,
        client_id: app.client_id,
        client_secret: app.client_secret,
    };
    let name = format!("github-{}", rec.source.name);
    let name = if name.len() <= 48 {
        name
    } else {
        format!("github-{id}")
    };
    let secret =
        sources::store_secret(&state, &name, &value, Some(format!("app {}", app.id))).await?;
    state.db.set_source_setup(&id, None, None).await?;
    let s = &mut rec.source;
    s.secret_id = Some(secret.id);
    s.app_id = Some(app.id);
    s.app_slug = Some(app.slug.clone());
    if let Some(owner) = app.owner {
        s.owner_kind = Some(owner.owner_kind());
        s.owner = Some(owner.login);
    }
    s.status = GitSourceStatus::NeedsInstall;
    s.status_detail = Some("install the app on the account and choose its repositories".into());
    state.db.save_source(&rec).await?;
    state
        .activity(
            ActivityLevel::Success,
            format!("GitHub App {} created", app.slug),
            Some(format!("source {}", rec.source.name)),
        )
        .await;
    Ok(Redirect::to(&github::install_url(&rec.source.html_url, &app.slug)).into_response())
}

#[derive(Deserialize)]
pub struct SetupQuery {
    installation_id: Option<u64>,
    #[serde(default)]
    setup_action: Option<String>,
}

/// GitHub sends the browser here after the app was installed or its repositories changed.
/// The installation only counts when the app's own JWT can read it.
pub async fn setup(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(q): Query<SetupQuery>,
) -> ApiResult<Response> {
    let mut rec = record(&state, &id).await?;
    let Some(installation_id) = q.installation_id else {
        let body = if q.setup_action.as_deref() == Some("request") {
            "<p>An owner of the organization has to approve the installation. Check the source again once they have.</p>"
        } else {
            "<p>GitHub did not say which installation this is. Install the app again from the source's install link.</p>"
        };
        return Ok(page("Waiting for the installation", body).into_response());
    };
    let jwt = sources::app_jwt(&state, &rec.source).await?;
    let installation = Github::new(&rec.source.api_url)?
        .installation(&jwt, installation_id)
        .await
        .map_err(|e| {
            AppError::BadRequest(format!(
                "this installation does not belong to the source's app: {e:#}"
            ))
        })?;
    let s = &mut rec.source;
    s.installation_id = Some(installation.id);
    if let Some(account) = installation.account {
        s.owner_kind = Some(account.owner_kind());
        s.owner = Some(account.login);
    }
    s.status = GitSourceStatus::Ready;
    s.status_detail = None;
    s.last_checked_at = Some(Utc::now());
    state.db.save_source(&rec).await?;
    state.repo_cache_forget(&id);
    let owner = rec.source.owner.clone().unwrap_or_default();
    state
        .activity(
            ActivityLevel::Success,
            format!("Git source {} installed", rec.source.name),
            Some(owner.clone()),
        )
        .await;
    let body = format!(
        "<p>Source {} can now clone from {}. You can close this tab and go back to the terminal.</p>",
        escape(&rec.source.name),
        escape(&owner)
    );
    Ok(page(&format!("{} is connected to GitHub", brand::NAME), &body).into_response())
}

/// Push events deploy every service that tracks the pushed branch with auto-deploy on, except
/// services with a root and watch paths that the push changed no file in.
pub async fn webhook(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Response> {
    let rec = state.db.source(&id).await?.ok_or(AppError::Unauthorized)?;
    if rec.source.kind != GitSourceKind::GithubApp {
        return Err(AppError::Unauthorized);
    }
    let creds = sources::app_credentials(&state, &rec.source).await?;
    let Some(secret) = creds.webhook_secret else {
        bad_request!("source '{}' has no webhook secret", rec.source.name);
    };
    let header_str = |name: &str| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string()
    };
    if !github::signature_matches(&secret, &body, &header_str("x-hub-signature-256")) {
        return Err(AppError::Unauthorized);
    }
    let parse_err = |e: serde_json::Error| AppError::BadRequest(format!("invalid event: {e}"));
    match header_str("x-github-event").as_str() {
        "push" => {
            let event: PushEvent = serde_json::from_slice(&body).map_err(parse_err)?;
            let n = deploy_push(&state, &rec.source, &event).await?;
            Ok((
                StatusCode::ACCEPTED,
                [(header::CONTENT_TYPE, "application/json")],
                serde_json::json!({ "deployments": n }).to_string(),
            )
                .into_response())
        }
        "installation" => {
            let event: InstallationEvent = serde_json::from_slice(&body).map_err(parse_err)?;
            installation_changed(&state, rec, event).await?;
            Ok(StatusCode::NO_CONTENT.into_response())
        }
        _ => Ok(StatusCode::NO_CONTENT.into_response()),
    }
}

async fn deploy_push(
    state: &Arc<AppState>,
    source: &GitSource,
    event: &PushEvent,
) -> ApiResult<usize> {
    let Some(branch) = event.branch().filter(|_| !event.deleted) else {
        return Ok(0);
    };
    let mut started = 0;
    for rec in state
        .db
        .services_tracking(&source.id, &event.repository.full_name)
        .await?
    {
        let Source::Git {
            branch: tracked,
            auto_deploy,
            root,
            watch_paths,
            ..
        } = &rec.service.source
        else {
            continue;
        };
        if tracked != branch || !auto_deploy {
            continue;
        }
        if *watch_paths && !event.touches(root) {
            tracing::info!(service = %rec.service.name, "push changed nothing in {root}, not deploying");
            continue;
        }
        match deploy::enqueue(state, &rec, "webhook", Plan::Source).await {
            Ok(_) => started += 1,
            Err(e) => tracing::warn!(service = %rec.service.name, "push deploy not started: {e}"),
        }
    }
    Ok(started)
}

async fn installation_changed(
    state: &AppState,
    mut rec: crate::db::SourceRecord,
    event: InstallationEvent,
) -> ApiResult<()> {
    if rec.source.installation_id != Some(event.installation.id) {
        return Ok(());
    }
    let s = &mut rec.source;
    match event.action.as_str() {
        "deleted" => {
            s.installation_id = None;
            s.status = GitSourceStatus::NeedsInstall;
            s.status_detail = Some("the app was uninstalled on GitHub; install it again".into());
        }
        "suspend" => {
            s.status = GitSourceStatus::Error;
            s.status_detail = Some("the installation was suspended on GitHub".into());
        }
        "unsuspend" => {
            s.status = GitSourceStatus::Ready;
            s.status_detail = None;
        }
        _ => return Ok(()),
    }
    state.db.save_source(&rec).await?;
    state.repo_cache_forget(&rec.source.id);
    state
        .activity(
            ActivityLevel::Info,
            format!(
                "Git source {} {}",
                rec.source.name,
                rec.source.status.as_str()
            ),
            rec.source.status_detail.clone(),
        )
        .await;
    Ok(())
}
