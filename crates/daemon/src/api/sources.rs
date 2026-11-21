//! Git sources: GitHub Apps (created through the manifest flow or connected) and personal
//! access tokens, plus the repository and branch pickers.

use super::{Json, Query, no_content};
use crate::db::{SourceRecord, new_id};
use crate::error::{ApiResult, AppError, bad_request, conflict};
use crate::github::{self, Github};
use crate::sources;
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use chrono::{Duration, Utc};
use common::api::{ConnectGithubApp, ConnectGithubToken, CreateGithubApp};
use common::*;
use serde::Deserialize;
use sha2::{Digest as _, Sha256};
use std::sync::Arc;

/// How long the setup URL of the manifest flow stays valid.
const SETUP_TTL: Duration = Duration::hours(1);

pub async fn list(State(state): State<Arc<AppState>>) -> ApiResult<Json<Vec<GitSource>>> {
    Ok(Json(
        state
            .db
            .sources()
            .await?
            .into_iter()
            .map(|r| sources::present(&state, r.source))
            .collect(),
    ))
}

pub async fn record(state: &AppState, id: &str) -> ApiResult<SourceRecord> {
    state
        .db
        .source(id)
        .await?
        .ok_or_else(|| AppError::not_found("git source", id))
}

pub async fn get(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<Json<GitSource>> {
    let rec = record(&state, &id).await?;
    Ok(Json(sources::present(&state, rec.source)))
}

/// The instance URL GitHub Apps call back to, or an error pointing at the domain guide.
fn require_domain(state: &AppState) -> ApiResult<String> {
    state.instance_url().ok_or_else(|| {
        AppError::BadRequest(format!(
            "GitHub Apps need a verified instance domain, because GitHub calls back to it: run `{} instance domain set <domain>` or open Settings, Domain in the TUI",
            brand::CLI_BIN
        ))
    })
}

async fn check_name(state: &AppState, name: &str) -> ApiResult<()> {
    validate_name(name).map_err(AppError::BadRequest)?;
    if state.db.source_name_taken(name).await? {
        conflict!("a git source named '{name}' already exists");
    }
    Ok(())
}

/// A GitHub login: letters, digits and single dashes.
fn validate_login(login: &str) -> ApiResult<()> {
    let ok = !login.is_empty()
        && login.len() <= 39
        && !login.starts_with('-')
        && login.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    if !ok {
        bad_request!("'{login}' is not a GitHub organization name");
    }
    Ok(())
}

fn new_record(
    name: String,
    kind: GitSourceKind,
    html_url: String,
    api_url: String,
    organization: Option<String>,
) -> SourceRecord {
    SourceRecord {
        organization,
        source: GitSource {
            id: new_id(),
            name,
            kind,
            html_url,
            api_url,
            owner: None,
            owner_kind: None,
            app_id: None,
            app_slug: None,
            installation_id: None,
            status: GitSourceStatus::Pending,
            status_detail: None,
            install_url: None,
            webhook_url: None,
            secret_id: None,
            last_checked_at: None,
            created_at: Utc::now(),
            used_by: Vec::new(),
        },
    }
}

pub fn hash_state(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

pub async fn create_github_app(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateGithubApp>,
) -> ApiResult<Json<GithubAppSetup>> {
    let instance = require_domain(&state)?;
    check_name(&state, &body.name).await?;
    let organization = body
        .organization
        .map(|o| o.trim().to_string())
        .filter(|o| !o.is_empty());
    if let Some(org) = &organization {
        validate_login(org)?;
    }
    let (html_url, api_url) = github::base_urls(body.html_url.as_deref(), body.api_url.as_deref())
        .map_err(AppError::BadRequest)?;
    let mut rec = new_record(
        body.name,
        GitSourceKind::GithubApp,
        html_url,
        api_url,
        organization,
    );
    rec.source.owner = rec.organization.clone();
    rec.source.owner_kind = rec
        .organization
        .as_ref()
        .map(|_| GitOwnerKind::Organization);
    rec.source.status_detail =
        Some("open the setup URL in a browser to create the app on GitHub".into());
    state.db.save_source(&rec).await?;
    let token = crate::config::generate_token();
    state
        .db
        .set_source_setup(
            &rec.source.id,
            Some(&hash_state(&token)),
            Some(Utc::now() + SETUP_TTL),
        )
        .await?;
    state
        .activity(
            ActivityLevel::Info,
            format!("Git source {} started", rec.source.name),
            Some(
                rec.organization
                    .as_ref()
                    .map_or("personal account".into(), |o| format!("organization {o}")),
            ),
        )
        .await;
    let setup_url = format!("{instance}/github/{}/start?state={token}", rec.source.id);
    Ok(Json(GithubAppSetup {
        source: sources::present(&state, rec.source),
        setup_url,
    }))
}

/// A secret name for a source's credentials that fits the name rules.
fn secret_name(source: &GitSource) -> String {
    let name = format!("github-{}", source.name);
    if name.len() <= 48 {
        name
    } else {
        format!("github-{}", source.id)
    }
}

/// Checks a freshly connected source against GitHub; a source that does not work is removed
/// again and the error returned.
async fn verify_new(state: &AppState, rec: SourceRecord) -> ApiResult<GitSource> {
    let id = rec.source.id.clone();
    let source = sources::check(state, rec).await?;
    if source.status == GitSourceStatus::Error {
        state.db.delete_source(&id).await?;
        bad_request!(
            "GitHub rejected the credentials: {}",
            source.status_detail.unwrap_or_default()
        );
    }
    state
        .activity(
            ActivityLevel::Success,
            format!("Git source {} connected", source.name),
            source.owner.clone(),
        )
        .await;
    Ok(source)
}

pub async fn connect_github_app(
    State(state): State<Arc<AppState>>,
    Json(body): Json<ConnectGithubApp>,
) -> ApiResult<Json<GitSource>> {
    require_domain(&state)?;
    check_name(&state, &body.name).await?;
    let private_key = format!("{}\n", body.private_key.trim());
    github::app_jwt(body.app_id, &private_key)
        .map_err(|e| AppError::BadRequest(format!("{e:#}")))?;
    let (html_url, api_url) = github::base_urls(body.html_url.as_deref(), body.api_url.as_deref())
        .map_err(AppError::BadRequest)?;
    let mut rec = new_record(body.name, GitSourceKind::GithubApp, html_url, api_url, None);
    rec.source.app_id = Some(body.app_id);
    rec.source.installation_id = body.installation_id;
    let value = SecretValue::GithubApp {
        app_id: body.app_id,
        private_key,
        webhook_secret: body.webhook_secret.filter(|s| !s.trim().is_empty()),
        client_id: None,
        client_secret: None,
    };
    let secret = sources::store_secret(
        &state,
        &secret_name(&rec.source),
        &value,
        Some(format!("app {}", body.app_id)),
    )
    .await?;
    rec.source.secret_id = Some(secret.id);
    state.db.save_source(&rec).await?;
    Ok(Json(verify_new(&state, rec).await?))
}

pub async fn connect_github_token(
    State(state): State<Arc<AppState>>,
    Json(body): Json<ConnectGithubToken>,
) -> ApiResult<Json<GitSource>> {
    check_name(&state, &body.name).await?;
    let token = body.token.trim().to_string();
    if token.is_empty() || token.chars().any(char::is_whitespace) {
        bad_request!("the token must be a single word");
    }
    let (html_url, api_url) = github::base_urls(body.html_url.as_deref(), body.api_url.as_deref())
        .map_err(AppError::BadRequest)?;
    let mut rec = new_record(
        body.name,
        GitSourceKind::GithubToken,
        html_url,
        api_url,
        None,
    );
    let value = SecretValue::Generic { value: token };
    let secret = sources::store_secret(&state, &secret_name(&rec.source), &value, None).await?;
    rec.source.secret_id = Some(secret.id);
    state.db.save_source(&rec).await?;
    Ok(Json(verify_new(&state, rec).await?))
}

pub async fn check(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<Json<GitSource>> {
    let rec = record(&state, &id).await?;
    Ok(Json(sources::check(&state, rec).await?))
}

pub async fn delete(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let rec = record(&state, &id).await?;
    if !rec.source.used_by.is_empty() {
        let users: Vec<String> = rec.source.used_by.iter().map(ToString::to_string).collect();
        conflict!(
            "git source '{}' is used by {}; change or delete them first",
            rec.source.name,
            users.join(", ")
        );
    }
    state.db.delete_source(&id).await?;
    state.repo_cache_forget(&id);
    let detail = (rec.source.kind == GitSourceKind::GithubApp)
        .then(|| rec.source.app_slug.clone())
        .flatten()
        .map(|slug| format!("delete the app {slug} on GitHub too if you no longer need it"));
    state
        .activity(
            ActivityLevel::Info,
            format!("Git source {} deleted", rec.source.name),
            detail,
        )
        .await;
    Ok(no_content())
}

/// A source that can list repositories.
async fn ready(state: &AppState, id: &str) -> ApiResult<GitSource> {
    let rec = record(state, id).await?;
    if rec.source.status != GitSourceStatus::Ready {
        bad_request!(
            "git source '{}' is {}: {}",
            rec.source.name,
            rec.source.status.as_str(),
            rec.source
                .status_detail
                .as_deref()
                .unwrap_or("check it first")
        );
    }
    Ok(rec.source)
}

#[derive(Deserialize)]
pub struct ReposQuery {
    #[serde(default)]
    q: String,
    page: Option<u32>,
    per_page: Option<u32>,
}

pub async fn repos(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(query): Query<ReposQuery>,
) -> ApiResult<Json<RepoPage>> {
    let source = ready(&state, &id).await?;
    let all = sources::repos(&state, &source).await?;
    Ok(Json(sources::page(
        &all,
        &query.q,
        query.page.unwrap_or(1),
        query.per_page.unwrap_or(30),
    )))
}

/// Entries of one kind (`file` or `dir`) directly inside `dir` on a branch.
async fn list_dir(
    state: &AppState,
    id: &str,
    repo: &str,
    branch: &str,
    dir: &str,
    kind: &str,
) -> ApiResult<Vec<String>> {
    github::validate_full_name(repo).map_err(|e| AppError::BadRequest(e.to_string()))?;
    let dir = clean_root(dir).map_err(AppError::BadRequest)?;
    let source = ready(state, id).await?;
    let token = sources::access_token(state, &source).await?;
    Ok(Github::new(&source.api_url)?
        .list_dir(&token, repo, branch, &dir, kind)
        .await?)
}

#[derive(Deserialize)]
pub struct DetectQuery {
    repo: String,
    branch: String,
    #[serde(default)]
    root: String,
}

/// The build method a branch would get, from the files in the service's root directory.
pub async fn detect(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(query): Query<DetectQuery>,
) -> ApiResult<Json<BuildDetection>> {
    let files = list_dir(&state, &id, &query.repo, &query.branch, &query.root, "file").await?;
    Ok(Json(crate::build::detect(
        |f| files.iter().any(|n| n == f),
        "Dockerfile",
        None,
    )))
}

#[derive(Deserialize)]
pub struct DirsQuery {
    repo: String,
    branch: String,
    #[serde(default)]
    path: String,
}

/// The directories directly under `path` on a branch, for picking a service's root.
pub async fn dirs(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(query): Query<DirsQuery>,
) -> ApiResult<Json<Vec<String>>> {
    let mut dirs = list_dir(&state, &id, &query.repo, &query.branch, &query.path, "dir").await?;
    dirs.sort();
    Ok(Json(dirs))
}

#[derive(Deserialize)]
pub struct BranchesQuery {
    repo: String,
}

pub async fn branches(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(query): Query<BranchesQuery>,
) -> ApiResult<Json<Vec<GitBranch>>> {
    github::validate_full_name(&query.repo).map_err(|e| AppError::BadRequest(e.to_string()))?;
    let source = ready(&state, &id).await?;
    let token = sources::access_token(&state, &source).await?;
    Ok(Json(
        Github::new(&source.api_url)?
            .branches(&token, &query.repo)
            .await?,
    ))
}
