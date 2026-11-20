//! API error type: maps failures to status codes and an `ApiError` JSON body.

use axum::Json;
use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use common::ApiError;

#[derive(Debug)]
pub enum AppError {
    BadRequest(String),
    Unauthorized,
    NotFound(String),
    Conflict(String),
    Internal(anyhow::Error),
}

pub type ApiResult<T> = Result<T, AppError>;

impl AppError {
    pub fn not_found(what: &str, id: &str) -> Self {
        Self::NotFound(format!("{what} '{id}' not found"))
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, error) = match self {
            Self::BadRequest(m) => (StatusCode::BAD_REQUEST, m),
            Self::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized".to_string()),
            Self::NotFound(m) => (StatusCode::NOT_FOUND, m),
            Self::Conflict(m) => (StatusCode::CONFLICT, m),
            Self::Internal(e) => {
                tracing::error!("request failed: {e:#}");
                (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}"))
            }
        };
        (status, Json(ApiError { error })).into_response()
    }
}

impl<E: Into<anyhow::Error>> From<E> for AppError {
    fn from(e: E) -> Self {
        Self::Internal(e.into())
    }
}

/// Bad request bodies get the same JSON error shape as every other failure.
pub fn json_rejection(r: JsonRejection) -> AppError {
    AppError::BadRequest(r.body_text())
}

pub fn query_rejection(r: QueryRejection) -> AppError {
    AppError::BadRequest(r.body_text())
}

/// Shorthand for `Err(AppError::BadRequest(..))`.
macro_rules! bad_request {
    ($($arg:tt)*) => {
        return Err($crate::error::AppError::BadRequest(format!($($arg)*)))
    };
}

/// Shorthand for `Err(AppError::Conflict(..))`.
macro_rules! conflict {
    ($($arg:tt)*) => {
        return Err($crate::error::AppError::Conflict(format!($($arg)*)))
    };
}

pub(crate) use {bad_request, conflict};
