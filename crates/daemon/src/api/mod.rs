//! HTTP API: router, bearer-token auth, JSON extractors and NDJSON streaming helpers.

mod deployments;
mod projects;
mod servers;
mod services;
mod system;

use crate::error::{AppError, json_rejection, query_rejection};
use crate::state::AppState;
use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::{FromRequest, FromRequestParts, Request, State};
use axum::http::{StatusCode, header, request::Parts};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use common::{API_PREFIX, LogLine};
use futures_util::{Stream, StreamExt as _};
use serde::de::DeserializeOwned;
use std::convert::Infallible;
use std::sync::Arc;

pub use servers::check_server;

pub fn router(state: Arc<AppState>) -> Router {
    let api = Router::new()
        .route("/info", get(system::info))
        .route("/tree", get(system::tree))
        .route("/activity", get(system::activity))
        .route("/servers", get(servers::list).post(servers::create))
        .route("/servers/{id}", delete(servers::delete))
        .route("/servers/{id}/check", post(servers::check))
        .route("/servers/{id}/proxy", post(servers::proxy))
        .route("/projects", get(projects::list).post(projects::create))
        .route("/projects/{id}", delete(projects::delete))
        .route(
            "/projects/{id}/environments",
            post(projects::create_environment),
        )
        .route("/environments/{id}", delete(projects::delete_environment))
        .route("/services", post(services::create))
        .route(
            "/services/{id}",
            get(services::get)
                .patch(services::update)
                .delete(services::delete),
        )
        .route("/services/{id}/deploy", post(services::deploy))
        .route("/services/{id}/restart", post(services::restart))
        .route("/services/{id}/stop", post(services::stop))
        .route("/services/{id}/start", post(services::start))
        .route("/services/{id}/logs", get(services::logs))
        .route("/services/{id}/stats", get(services::stats))
        .route("/services/{id}/env", put(services::set_env))
        .route("/services/{id}/domains", post(services::add_domain))
        .route("/domains/{id}", delete(services::delete_domain))
        .route("/services/{id}/volumes", post(services::add_volume))
        .route("/volumes/{id}", delete(services::delete_volume))
        .route("/services/{id}/deployments", get(deployments::list))
        .route("/deployments/{id}", get(deployments::get))
        .route("/deployments/{id}/logs", get(deployments::logs))
        .route("/deployments/{id}/cancel", post(deployments::cancel))
        .route("/deployments/{id}/rollback", post(deployments::rollback))
        .route_layer(middleware::from_fn_with_state(state.clone(), auth));
    Router::new()
        .nest(API_PREFIX, api)
        .route("/healthz", get(|| async { "ok" }))
        .fallback(|| async { AppError::NotFound("no such route".into()) })
        .with_state(state)
}

async fn auth(State(state): State<Arc<AppState>>, req: Request, next: Next) -> Response {
    let authorized = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .is_some_and(|token| state.token_matches(token.trim()));
    if !authorized {
        return AppError::Unauthorized.into_response();
    }
    next.run(req).await
}

/// `axum::Json` whose rejection is an `ApiError` body.
pub struct Json<T>(pub T);

impl<S: Send + Sync, T: DeserializeOwned> FromRequest<S> for Json<T> {
    type Rejection = AppError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        axum::Json::<T>::from_request(req, state)
            .await
            .map(|axum::Json(v)| Self(v))
            .map_err(json_rejection)
    }
}

impl<T: serde::Serialize> IntoResponse for Json<T> {
    fn into_response(self) -> Response {
        axum::Json(self.0).into_response()
    }
}

/// `axum::extract::Query` whose rejection is an `ApiError` body.
pub struct Query<T>(pub T);

impl<S: Send + Sync, T: DeserializeOwned> FromRequestParts<S> for Query<T> {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        axum::extract::Query::<T>::from_request_parts(parts, state)
            .await
            .map(|axum::extract::Query(v)| Self(v))
            .map_err(query_rejection)
    }
}

/// Parses an optional JSON body: empty means `T::default()`.
fn optional_json<T: DeserializeOwned + Default>(body: &Bytes) -> Result<T, AppError> {
    if body.iter().all(u8::is_ascii_whitespace) {
        return Ok(T::default());
    }
    serde_json::from_slice(body)
        .map_err(|e| AppError::BadRequest(format!("invalid JSON body: {e}")))
}

pub fn no_content() -> StatusCode {
    StatusCode::NO_CONTENT
}

fn ndjson_line(line: &LogLine) -> Bytes {
    let mut buf = serde_json::to_vec(line).expect("log lines always serialize");
    buf.push(b'\n');
    Bytes::from(buf)
}

/// Streams log lines as newline-delimited JSON.
fn ndjson(lines: impl Stream<Item = LogLine> + Send + 'static) -> Response {
    let body = Body::from_stream(lines.map(|l| Ok::<_, Infallible>(ndjson_line(&l))));
    ([(header::CONTENT_TYPE, "application/x-ndjson")], body).into_response()
}
