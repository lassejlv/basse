//! Notification channels, rules and the delivery log.

use super::{Json, Query, no_content};
use crate::db::new_id;
use crate::error::{ApiResult, AppError, bad_request, conflict};
use crate::notify::{self, channel_aad};
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use chrono::Utc;
use common::api::{CreateNotifyChannel, CreateNotifyRule, UpdateNotifyChannel, UpdateNotifyRule};
use common::notify::*;
use common::{ActivityLevel, validate_name};
use serde::Deserialize;
use std::sync::Arc;
use zeroize::Zeroizing;

pub async fn catalog() -> Json<NotifyCatalog> {
    Json(NotifyCatalog::builtin())
}

pub async fn list_channels(
    State(state): State<Arc<AppState>>,
) -> ApiResult<Json<Vec<NotifyChannel>>> {
    Ok(Json(state.db.notify_channels().await?))
}

pub async fn get_channel(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<Json<NotifyChannel>> {
    Ok(Json(channel(&state, &id).await?))
}

async fn channel(state: &AppState, id: &str) -> ApiResult<NotifyChannel> {
    state
        .db
        .notify_channel(id)
        .await?
        .ok_or_else(|| AppError::not_found("notification channel", id))
}

/// The full config sealed for channel `id`.
fn seal(state: &AppState, id: &str, config: &ChannelConfig) -> ApiResult<Vec<u8>> {
    config.validate().map_err(AppError::BadRequest)?;
    let json = Zeroizing::new(serde_json::to_vec(config).map_err(anyhow::Error::from)?);
    if String::from_utf8_lossy(&json).contains(&format!("\"{REDACTED}\"")) {
        bad_request!("a credential was sent as {REDACTED} but there is no stored value to keep");
    }
    Ok(state.secrets.key_seal(&channel_aad(id), &json)?)
}

pub async fn create_channel(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateNotifyChannel>,
) -> ApiResult<Json<NotifyChannel>> {
    validate_name(&body.name).map_err(AppError::BadRequest)?;
    if state.db.notify_channel_name_taken(&body.name).await? {
        conflict!(
            "a notification channel named '{}' already exists",
            body.name
        );
    }
    let id = new_id();
    let sealed = seal(&state, &id, &body.config)?;
    let channel = NotifyChannel {
        id,
        name: body.name,
        kind: body.config.kind(),
        enabled: body.enabled,
        target: body.config.target(),
        config: body.config.redacted(),
        created_at: Utc::now(),
        last_test: None,
    };
    state
        .db
        .save_notify_channel(&channel, Some(&sealed))
        .await?;
    state
        .activity(
            ActivityLevel::Info,
            format!("Notification channel {} added", channel.name),
            Some(channel.kind.label().to_string()),
        )
        .await;
    Ok(Json(channel))
}

pub async fn update_channel(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<UpdateNotifyChannel>,
) -> ApiResult<Json<NotifyChannel>> {
    let mut channel = channel(&state, &id).await?;
    if let Some(name) = body.name.filter(|n| *n != channel.name) {
        validate_name(&name).map_err(AppError::BadRequest)?;
        if state.db.notify_channel_name_taken(&name).await? {
            conflict!("a notification channel named '{name}' already exists");
        }
        channel.name = name;
    }
    if let Some(enabled) = body.enabled {
        channel.enabled = enabled;
    }
    let sealed = match body.config {
        Some(config) => {
            let config = config.keep_redacted(&state.channel_config(&id).await?);
            let sealed = seal(&state, &id, &config)?;
            channel.kind = config.kind();
            channel.target = config.target();
            channel.config = config.redacted();
            channel.last_test = None;
            Some(sealed)
        }
        None => None,
    };
    state
        .db
        .save_notify_channel(&channel, sealed.as_deref())
        .await?;
    Ok(Json(channel))
}

pub async fn delete_channel(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let channel = channel(&state, &id).await?;
    state.db.delete_notify_channel(&id).await?;
    state
        .activity(
            ActivityLevel::Info,
            format!("Notification channel {} removed", channel.name),
            None,
        )
        .await;
    Ok(no_content())
}

pub async fn test_channel(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<Json<NotifyDelivery>> {
    let channel = channel(&state, &id).await?;
    Ok(Json(notify::test_channel(&state, channel).await?))
}

/// `instance`, `acme`, `acme/production` or `acme/production/api`; 404 when the scope's target
/// does not exist.
async fn scope_label(state: &AppState, scope: &RuleScope) -> ApiResult<String> {
    let project = |id: String| async move {
        state
            .db
            .project(&id)
            .await?
            .ok_or_else(|| AppError::not_found("project", &id))
    };
    let environment = |id: String| async move {
        let env = state
            .db
            .environment(&id)
            .await?
            .ok_or_else(|| AppError::not_found("environment", &id))?;
        ApiResult::Ok(env)
    };
    Ok(match scope {
        RuleScope::Instance => "instance".into(),
        RuleScope::Project { id } => project(id.clone()).await?.name,
        RuleScope::Environment { id } => {
            let env = environment(id.clone()).await?;
            format!("{}/{}", project(env.project_id).await?.name, env.name)
        }
        RuleScope::Service { id } => {
            let rec = state
                .db
                .service(id)
                .await?
                .ok_or_else(|| AppError::not_found("service", id))?;
            let env = environment(rec.service.environment_id.clone()).await?;
            format!(
                "{}/{}/{}",
                project(env.project_id).await?.name,
                env.name,
                rec.service.name
            )
        }
    })
}

async fn labelled(state: &AppState, mut rule: NotifyRule) -> ApiResult<NotifyRule> {
    rule.scope_label = scope_label(state, &rule.scope)
        .await
        .unwrap_or_else(|_| "(deleted)".into());
    Ok(rule)
}

pub async fn list_rules(State(state): State<Arc<AppState>>) -> ApiResult<Json<Vec<NotifyRule>>> {
    let mut out = Vec::new();
    for rule in state.db.notify_rules().await? {
        out.push(labelled(&state, rule).await?);
    }
    Ok(Json(out))
}

async fn rule(state: &AppState, id: &str) -> ApiResult<NotifyRule> {
    state
        .db
        .notify_rule(id)
        .await?
        .ok_or_else(|| AppError::not_found("notification rule", id))
}

fn check_events(events: &mut Vec<NotifyEvent>) -> ApiResult<()> {
    events.sort();
    events.dedup();
    if events.is_empty() {
        bad_request!("a rule needs at least one event");
    }
    Ok(())
}

/// A known preset id, or `None` for a custom pick (empty or `custom`).
fn check_preset(preset: Option<String>) -> ApiResult<Option<String>> {
    match preset.filter(|p| !p.is_empty() && p != "custom") {
        Some(p) if common::notify::preset(&p).is_none() => {
            let ids: Vec<_> = PRESETS.iter().map(|p| p.id).collect();
            bad_request!("unknown preset '{p}', expected one of {}", ids.join(", "))
        }
        other => Ok(other),
    }
}

pub async fn create_rule(
    State(state): State<Arc<AppState>>,
    Json(mut body): Json<CreateNotifyRule>,
) -> ApiResult<Json<NotifyRule>> {
    channel(&state, &body.channel_id).await?;
    check_events(&mut body.events)?;
    let scope_label = scope_label(&state, &body.scope).await?;
    let rule = NotifyRule {
        id: new_id(),
        channel_id: body.channel_id,
        scope: body.scope,
        scope_label,
        preset: check_preset(body.preset)?,
        events: body.events,
        enabled: body.enabled,
        dedupe_secs: body.dedupe_secs.unwrap_or(DEFAULT_DEDUPE_SECS),
        max_per_hour: body.max_per_hour.unwrap_or(DEFAULT_MAX_PER_HOUR),
        created_at: Utc::now(),
    };
    state.db.save_notify_rule(&rule).await?;
    Ok(Json(rule))
}

pub async fn update_rule(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<UpdateNotifyRule>,
) -> ApiResult<Json<NotifyRule>> {
    let mut rule = rule(&state, &id).await?;
    if let Some(channel_id) = body.channel_id {
        channel(&state, &channel_id).await?;
        rule.channel_id = channel_id;
    }
    if let Some(scope) = body.scope {
        scope_label(&state, &scope).await?;
        rule.scope = scope;
    }
    if let Some(preset) = body.preset {
        rule.preset = check_preset(Some(preset))?;
    }
    if let Some(mut events) = body.events {
        check_events(&mut events)?;
        rule.events = events;
    }
    if let Some(enabled) = body.enabled {
        rule.enabled = enabled;
    }
    if let Some(secs) = body.dedupe_secs {
        rule.dedupe_secs = secs;
    }
    if let Some(max) = body.max_per_hour {
        rule.max_per_hour = max;
    }
    state.db.save_notify_rule(&rule).await?;
    Ok(Json(labelled(&state, rule).await?))
}

pub async fn delete_rule(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    rule(&state, &id).await?;
    state.db.delete_notify_rule(&id).await?;
    Ok(no_content())
}

#[derive(Debug, Deserialize)]
pub struct DeliveryQuery {
    #[serde(default = "default_limit")]
    limit: usize,
    channel_id: Option<String>,
    rule_id: Option<String>,
}

fn default_limit() -> usize {
    50
}

pub async fn deliveries(
    State(state): State<Arc<AppState>>,
    Query(q): Query<DeliveryQuery>,
) -> ApiResult<Json<Vec<NotifyDelivery>>> {
    Ok(Json(
        state
            .db
            .deliveries(
                q.limit.min(1000),
                q.channel_id.as_deref().filter(|s| !s.is_empty()),
                q.rule_id.as_deref().filter(|s| !s.is_empty()),
            )
            .await?,
    ))
}
