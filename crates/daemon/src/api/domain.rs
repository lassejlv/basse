//! The instance domain: set it, see the DNS records to create, and check it end to end.

use super::{Json, servers};
use crate::domain;
use crate::error::{ApiResult, AppError, bad_request, conflict};
use crate::state::AppState;
use axum::extract::State;
use chrono::Utc;
use common::api::SetInstanceDomain;
use common::*;
use std::net::IpAddr;
use std::sync::Arc;

pub async fn get(State(state): State<Arc<AppState>>) -> Json<InstanceDomain> {
    Json(status(&state).await)
}

async fn status(state: &AppState) -> InstanceDomain {
    let config = state.config();
    let (v4, v6) = state.public_ips().await;
    let last = state
        .domain_checks
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .clone()
        .filter(|(d, _, _)| config.domain.as_deref() == Some(d.as_str()))
        .map(|(_, checks, at)| (checks, at));
    domain::status(
        config.domain.as_deref(),
        config.domain_verified_at,
        v4,
        v6,
        last,
    )
}

pub async fn set(
    State(state): State<Arc<AppState>>,
    Json(body): Json<SetInstanceDomain>,
) -> ApiResult<Json<InstanceDomain>> {
    let name = domain::normalize(&body.domain).map_err(AppError::BadRequest)?;
    if state.db.domain_taken(&name).await? {
        conflict!("{name} is already a domain of a service; use another hostname for the instance");
    }
    let (_, changed) = state.update_config(|c| {
        let changed = c.domain.as_deref() != Some(name.as_str());
        if changed {
            c.domain = Some(name.clone());
            c.domain_verified_at = None;
        }
        Ok(changed)
    })?;
    if changed {
        state
            .activity(
                ActivityLevel::Info,
                "Instance domain set".into(),
                Some(name.clone()),
            )
            .await;
    }
    route(&state).await?;
    Ok(Json(run_check(&state).await?))
}

pub async fn check(State(state): State<Arc<AppState>>) -> ApiResult<Json<InstanceDomain>> {
    Ok(Json(run_check(&state).await?))
}

pub async fn remove(State(state): State<Arc<AppState>>) -> ApiResult<Json<InstanceDomain>> {
    let (_, old) = state.update_config(|c| {
        c.domain_verified_at = None;
        Ok(c.domain.take())
    })?;
    if let Some(old) = old {
        state
            .activity(
                ActivityLevel::Info,
                "Instance domain removed".into(),
                Some(old),
            )
            .await;
        route(&state).await?;
    }
    Ok(Json(status(&state).await))
}

/// Reinstalls the local proxy so it serves the current domain, when the daemon manages the
/// proxy or one is already running.
async fn route(state: &AppState) -> ApiResult<()> {
    let Some(local) = state.db.local_server().await? else {
        return Ok(());
    };
    if state.config().manage_proxy || local.proxy == ProxyStatus::Running {
        servers::install_proxy(state, local).await?;
    }
    Ok(())
}

async fn run_check(state: &AppState) -> ApiResult<InstanceDomain> {
    let Some(name) = state.config().domain else {
        bad_request!(
            "no instance domain is set: run `{} instance domain set <domain>`",
            brand::CLI_BIN
        );
    };
    let (v4, v6) = state.public_ips().await;
    let public: Vec<IpAddr> = [v4, v6].into_iter().flatten().collect();
    let checks = domain::run_checks(state.probe.as_ref(), &name, &public).await;
    let ok = domain::all_passed(&checks);
    let (_, was) = state.update_config(|c| {
        let was = c.domain_verified_at.is_some();
        if c.domain.as_deref() == Some(name.as_str()) {
            c.domain_verified_at = ok.then(|| c.domain_verified_at.unwrap_or_else(Utc::now));
        }
        Ok(was)
    })?;
    *state
        .domain_checks
        .lock()
        .unwrap_or_else(|p| p.into_inner()) = Some((name.clone(), checks.clone(), Utc::now()));
    match (was, ok) {
        (false, true) => {
            state
                .activity(
                    ActivityLevel::Success,
                    format!("Instance domain {name} verified"),
                    Some(format!("https://{name}")),
                )
                .await
        }
        (true, false) => {
            let failed = checks
                .iter()
                .find(|c| c.status == CheckStatus::Fail)
                .map(|c| format!("{}: {}", c.kind.label(), c.detail));
            state
                .activity(
                    ActivityLevel::Error,
                    format!("Instance domain {name} no longer verifies"),
                    failed,
                )
                .await
        }
        _ => {}
    }
    Ok(status(state).await)
}
