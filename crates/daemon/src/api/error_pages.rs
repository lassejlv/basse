//! Custom error pages of services and the instance defaults.

use super::{Json, no_content};
use crate::error::{ApiResult, AppError};
use crate::error_pages;
use crate::services;
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use common::api::SetErrorPage;
use common::*;
use std::sync::Arc;

fn kind(raw: &str) -> ApiResult<ErrorPageKind> {
    raw.parse().map_err(AppError::BadRequest)
}

pub async fn set_service(
    State(state): State<Arc<AppState>>,
    Path((id, page)): Path<(String, String)>,
    Json(body): Json<SetErrorPage>,
) -> ApiResult<Json<CustomErrorPage>> {
    let kind = kind(&page)?;
    let rec = services::record(&state, &id).await?;
    Ok(Json(
        error_pages::set(&state, Some(&rec), kind, body).await?,
    ))
}

pub async fn delete_service(
    State(state): State<Arc<AppState>>,
    Path((id, page)): Path<(String, String)>,
) -> ApiResult<StatusCode> {
    let kind = kind(&page)?;
    let rec = services::record(&state, &id).await?;
    error_pages::delete(&state, Some(&rec), kind).await?;
    Ok(no_content())
}

pub async fn instance(State(state): State<Arc<AppState>>) -> ApiResult<Json<ErrorPages>> {
    Ok(Json(error_pages::of_instance(&state).await?))
}

pub async fn set_instance(
    State(state): State<Arc<AppState>>,
    Path(page): Path<String>,
    Json(body): Json<SetErrorPage>,
) -> ApiResult<Json<CustomErrorPage>> {
    Ok(Json(
        error_pages::set(&state, None, kind(&page)?, body).await?,
    ))
}

pub async fn delete_instance(
    State(state): State<Arc<AppState>>,
    Path(page): Path<String>,
) -> ApiResult<StatusCode> {
    error_pages::delete(&state, None, kind(&page)?).await?;
    Ok(no_content())
}
