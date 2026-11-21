//! Secrets: SSH keys, S3 credentials and opaque values, encrypted at rest.

use super::{Json, no_content};
use crate::db::new_id;
use crate::error::{ApiResult, AppError, bad_request, conflict};
use crate::secrets;
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use chrono::Utc;
use common::api::{CreateSecret, GenerateSshKey, UpdateSecret};
use common::*;
use std::sync::Arc;

pub async fn list(State(state): State<Arc<AppState>>) -> ApiResult<Json<Vec<Secret>>> {
    Ok(Json(state.db.secrets().await?))
}

pub async fn get(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<Json<Secret>> {
    Ok(Json(secret(&state, &id).await?))
}

pub async fn secret(state: &AppState, id: &str) -> ApiResult<Secret> {
    state
        .db
        .secret(id)
        .await?
        .ok_or_else(|| AppError::not_found("secret", id))
}

/// A secret that must be of `kind`, for routes that take a secret id.
pub async fn secret_of_kind(state: &AppState, id: &str, kind: SecretKind) -> ApiResult<Secret> {
    let secret = secret(state, id).await?;
    if secret.kind != kind {
        bad_request!(
            "secret '{}' is {}, expected {}",
            secret.name,
            secret.kind.label(),
            kind.label()
        );
    }
    Ok(secret)
}

async fn check_name(state: &AppState, name: &str) -> ApiResult<()> {
    validate_name(name).map_err(AppError::BadRequest)?;
    if state.db.secret_name_taken(name).await? {
        conflict!("a secret named '{name}' already exists");
    }
    Ok(())
}

async fn insert(state: &AppState, name: String, value: SecretValue) -> ApiResult<Secret> {
    let value = secrets::normalize(value);
    let public_part = secrets::validate(&value).map_err(AppError::BadRequest)?;
    let now = Utc::now();
    let secret = Secret {
        id: new_id(),
        name,
        kind: value.kind(),
        created_at: now,
        updated_at: now,
        public_part,
        used_by: Vec::new(),
    };
    let sealed = state.secrets.seal(&secret.id, &value)?;
    state.db.insert_secret(&secret, &sealed).await?;
    state
        .activity(
            ActivityLevel::Info,
            format!("Secret {} added", secret.name),
            Some(secret.kind.label().to_string()),
        )
        .await;
    Ok(secret)
}

pub async fn create(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateSecret>,
) -> ApiResult<Json<Secret>> {
    check_name(&state, &body.name).await?;
    Ok(Json(insert(&state, body.name, body.value).await?))
}

pub async fn generate_ssh_key(
    State(state): State<Arc<AppState>>,
    Json(body): Json<GenerateSshKey>,
) -> ApiResult<Json<Secret>> {
    check_name(&state, &body.name).await?;
    let comment = format!("{}@{}", body.name, crate::host::hostname());
    let private_key = secrets::generate_ssh_key(&comment)?;
    let value = SecretValue::SshKey {
        private_key: private_key.to_string(),
    };
    Ok(Json(insert(&state, body.name, value).await?))
}

pub async fn update(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<UpdateSecret>,
) -> ApiResult<Json<Secret>> {
    let mut secret = secret(&state, &id).await?;
    let old_name = secret.name.clone();
    if let Some(name) = body.name.filter(|n| *n != secret.name) {
        check_name(&state, &name).await?;
        secret.name = name;
    }
    let sealed = match body.value {
        Some(value) => {
            if value.kind() != secret.kind {
                bad_request!(
                    "secret '{}' holds {}; a new value must be of the same kind",
                    secret.name,
                    secret.kind.label()
                );
            }
            let value = secrets::normalize(value);
            secret.public_part = secrets::validate(&value).map_err(AppError::BadRequest)?;
            Some(state.secrets.seal(&secret.id, &value)?)
        }
        None => None,
    };
    secret.updated_at = Utc::now();
    state.db.update_secret(&secret, sealed.as_deref()).await?;
    if sealed.is_some() {
        for usage in secret
            .used_by
            .iter()
            .filter(|u| u.kind == UsageKind::Server)
        {
            state.docker.forget(&usage.id);
        }
    }
    let message = match (old_name != secret.name, sealed.is_some()) {
        (true, true) => format!("Secret {old_name} renamed to {} and replaced", secret.name),
        (true, false) => format!("Secret {old_name} renamed to {}", secret.name),
        (false, _) => format!("Secret {} replaced", secret.name),
    };
    state.activity(ActivityLevel::Info, message, None).await;
    Ok(Json(secret))
}

pub async fn delete(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let secret = secret(&state, &id).await?;
    if !secret.used_by.is_empty() {
        let users: Vec<String> = secret.used_by.iter().map(ToString::to_string).collect();
        conflict!(
            "secret '{}' is used by {}; change them first",
            secret.name,
            users.join(", ")
        );
    }
    state.db.delete_secret(&id).await?;
    state
        .activity(
            ActivityLevel::Info,
            format!("Secret {} deleted", secret.name),
            None,
        )
        .await;
    Ok(no_content())
}

pub async fn reveal(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<Json<RevealedSecret>> {
    let secret = secret(&state, &id).await?;
    if secret.kind == SecretKind::GithubApp {
        bad_request!(
            "secret '{}' holds GitHub App credentials, which are never revealed",
            secret.name
        );
    }
    let value = state.secrets.value(&secret.id).await?;
    state
        .activity(
            ActivityLevel::Info,
            format!("Secret {} revealed", secret.name),
            None,
        )
        .await;
    Ok(Json(RevealedSecret { secret, value }))
}
