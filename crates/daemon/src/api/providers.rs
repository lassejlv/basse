//! Provider connections and the services' managed load balancers.

use super::{Json, Query};
use crate::error::{ApiResult, AppError, bad_request, conflict};
use crate::lb;
use crate::services;
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use chrono::Utc;
use common::api::{LbDisableQuery, LbSyncQuery};
use common::lb::*;
use common::*;
use std::sync::Arc;

pub async fn list(State(state): State<Arc<AppState>>) -> ApiResult<Json<Vec<Provider>>> {
    Ok(Json(state.db.providers().await?))
}

async fn find(state: &AppState, id: &str) -> ApiResult<Provider> {
    state
        .db
        .provider(id)
        .await?
        .ok_or_else(|| AppError::not_found("provider", id))
}

pub async fn get(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<Json<Provider>> {
    Ok(Json(find(&state, &id).await?))
}

/// Stores the connection after the provider accepted the token. A pasted token becomes a
/// `generic` secret named after the connection.
pub async fn create(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateProvider>,
) -> ApiResult<Json<Provider>> {
    validate_name(&body.name).map_err(AppError::BadRequest)?;
    if state.db.provider_name_taken(&body.name).await? {
        conflict!("a provider named '{}' already exists", body.name);
    }
    let (secret_id, created) = match (&body.token, &body.token_secret_id) {
        (Some(_), Some(_)) => bad_request!("send either token or token_secret_id, not both"),
        (None, None) => {
            bad_request!("send the API token, or the id of a generic secret holding it")
        }
        (None, Some(id)) => {
            super::secrets::secret_of_kind(&state, id, SecretKind::Generic).await?;
            (id.clone(), false)
        }
        (Some(token), None) => {
            let token = token.trim();
            if token.is_empty() {
                bad_request!("the API token is empty");
            }
            let name = format!("{}-token", body.name);
            super::secrets::check_name(&state, &name).await?;
            let secret = super::secrets::insert(
                &state,
                name,
                SecretValue::Generic {
                    value: token.to_string(),
                },
            )
            .await?;
            (secret.id, true)
        }
    };
    let mut provider = Provider {
        id: crate::db::new_id(),
        name: body.name,
        kind: body.kind,
        token_secret_id: secret_id.clone(),
        status: ProviderStatus::Unknown,
        problem: None,
        account: None,
        load_balancing: false,
        last_checked_at: None,
        created_at: Utc::now(),
    };
    check(&state, &mut provider).await;
    if provider.status != ProviderStatus::Connected {
        if created {
            let _ = state.db.delete_secret(&secret_id).await;
        }
        bad_request!(
            "{} refused the token: {}",
            provider.kind.label(),
            provider.problem.unwrap_or_default()
        );
    }
    state.db.save_provider(&provider).await?;
    state
        .activity(
            ActivityLevel::Info,
            format!("Provider {} connected", provider.name),
            provider.account.clone(),
        )
        .await;
    Ok(Json(provider))
}

/// Asks the provider who the token is, and records the answer on `provider`.
async fn check(state: &AppState, provider: &mut Provider) {
    let result = async {
        let token = lb::token(state, provider).await?;
        match provider.kind {
            ProviderKind::Hetzner => {
                let servers = state.lb.hetzner(token).servers().await?;
                provider.account = Some(format!(
                    "{} server{}",
                    servers.len(),
                    if servers.len() == 1 { "" } else { "s" }
                ));
            }
            ProviderKind::Cloudflare => {
                let cf = state.lb.cloudflare(token);
                cf.verify().await?;
                let accounts = cf.accounts().await?;
                let mut lb = false;
                for (id, _) in &accounts {
                    lb |= cf.has_load_balancing(id).await?;
                }
                provider.load_balancing = lb;
                let names: Vec<&str> = accounts.iter().map(|(_, n)| n.as_str()).collect();
                provider.account = Some(names.join(", "));
            }
        }
        anyhow::Ok(())
    }
    .await;
    provider.last_checked_at = Some(Utc::now());
    match result {
        Ok(()) => {
            provider.status = ProviderStatus::Connected;
            provider.problem = None;
        }
        Err(e) => {
            provider.status = ProviderStatus::Error;
            provider.problem = Some(format!("{e:#}"));
        }
    }
}

pub async fn check_route(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<Json<Provider>> {
    let mut provider = find(&state, &id).await?;
    check(&state, &mut provider).await;
    state.db.save_provider(&provider).await?;
    Ok(Json(provider))
}

/// What the token sees: Hetzner servers matched to the daemon's by address, or Cloudflare zones.
pub async fn discover(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<Json<ProviderDiscovery>> {
    let provider = find(&state, &id).await?;
    let token = lb::token(&state, &provider).await?;
    let api = |e: anyhow::Error| AppError::BadRequest(format!("{e:#}"));
    let mut out = ProviderDiscovery::default();
    match provider.kind {
        ProviderKind::Hetzner => {
            let hz = state.lb.hetzner(token);
            let servers = hz.servers().await.map_err(api)?;
            for server in state.db.servers().await? {
                let (v4, _) = state.server_ips(&server).await;
                let address = v4.map(|ip| ip.to_string());
                let hetzner = servers
                    .iter()
                    .find(|h| {
                        address.is_some()
                            && (h.public_ipv4 == address
                                || h.private_networks
                                    .iter()
                                    .any(|p| Some(&p.ip) == address.as_ref()))
                    })
                    .cloned();
                out.servers.push(ServerMatch {
                    server_id: server.id,
                    address,
                    hetzner,
                });
            }
            out.locations = hz.locations().await.map_err(api)?;
            out.lb_types = hz.lb_types().await.map_err(api)?;
            out.networks = hz.networks().await.map_err(api)?;
            out.load_balancers = hz
                .load_balancers()
                .await
                .map_err(api)?
                .iter()
                .map(|l| l.summary(l.labels.contains_key(&brand::label("managed"))))
                .collect();
        }
        ProviderKind::Cloudflare => {
            out.zones = state.lb.cloudflare(token).zones().await.map_err(api)?;
        }
    }
    Ok(Json(out))
}

pub async fn delete(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let provider = find(&state, &id).await?;
    let users = state.db.provider_users(&id).await?;
    if !users.is_empty() {
        conflict!(
            "{} is used by the load balancer of {}; turn those off first",
            provider.name,
            users.join(", ")
        );
    }
    state.db.delete_provider(&id).await?;
    state
        .activity(
            ActivityLevel::Info,
            format!("Provider {} removed", provider.name),
            Some("its token stays in Secrets".into()),
        )
        .await;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn service_lb(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<Json<ServiceLb>> {
    let rec = services::record(&state, &id).await?;
    match state.db.service_lb(&id).await? {
        Some(lb) => Ok(Json(lb::to_api(&lb))),
        None => Err(AppError::NotFound(format!(
            "'{}' has no load balancer",
            rec.service.name
        ))),
    }
}

pub async fn plan(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(config): Json<LbConfig>,
) -> ApiResult<Json<LbPlan>> {
    let rec = services::record(&state, &id).await?;
    Ok(Json(lb::plan(&state, &rec, &config).await?))
}

pub async fn enable(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<EnableLb>,
) -> ApiResult<Json<ServiceLb>> {
    let rec = services::record(&state, &id).await?;
    Ok(Json(lb::enable(&state, &rec, body).await?))
}

pub async fn sync(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(q): Query<LbSyncQuery>,
) -> ApiResult<Json<ServiceLb>> {
    let rec = services::record(&state, &id).await?;
    match lb::sync(&state, &id, q.force).await? {
        Some(lb) => Ok(Json(lb)),
        None => Err(AppError::NotFound(format!(
            "'{}' has no load balancer",
            rec.service.name
        ))),
    }
}

pub async fn disable(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(q): Query<LbDisableQuery>,
) -> ApiResult<Json<LbTeardown>> {
    let rec = services::record(&state, &id).await?;
    Ok(Json(lb::disable(&state, &rec, q.delete_resources).await?))
}
