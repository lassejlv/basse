//! HTTP API: router, bearer-token auth, JSON extractors and NDJSON streaming helpers.

mod databases;
mod deployments;
mod domain;
mod github;
mod instance;
mod notify;
mod onboarding;
mod projects;
mod secrets;
mod servers;
mod services;
mod settings;
mod sources;
mod storage;
mod system;

use crate::error::{AppError, json_rejection, query_rejection};
use crate::state::AppState;
use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::{FromRequest, FromRequestParts, Request, State};
use axum::http::{StatusCode, header, request::Parts};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, patch, post, put};
use common::{API_PREFIX, LogLine};
use futures_util::{Stream, StreamExt as _};
use serde::de::DeserializeOwned;
use std::convert::Infallible;
use std::sync::Arc;

pub use servers::{check_server, ensure_local_proxy, sync_proxy};
pub use settings::latest_version;
pub use storage::run_storage_test;

pub fn router(state: Arc<AppState>) -> Router {
    let api = Router::new()
        .route("/info", get(system::info))
        .route("/tree", get(system::tree))
        .route("/activity", get(system::activity))
        .route(
            "/onboarding",
            get(onboarding::get).patch(onboarding::update),
        )
        .route("/servers", get(servers::list).post(servers::create))
        .route(
            "/servers/{id}",
            patch(servers::update).delete(servers::delete),
        )
        .route("/servers/{id}/check", post(servers::check))
        .route("/servers/{id}/proxy", post(servers::proxy))
        .route("/servers/{id}/docker", post(servers::install_docker))
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
        .route("/services/{id}/move", post(services::relocate))
        .route("/services/{id}/restart", post(services::restart))
        .route("/services/{id}/stop", post(services::stop))
        .route("/services/{id}/start", post(services::start))
        .route("/services/{id}/logs", get(services::logs))
        .route("/services/{id}/stats", get(services::stats))
        .route("/services/{id}/metrics", get(services::metrics))
        .route("/services/{id}/scale", post(services::scale))
        .route("/services/{id}/autoscale", put(services::autoscale))
        .route("/services/{id}/capacity", get(services::capacity))
        .route("/services/{id}/env", put(services::set_env))
        .route("/services/{id}/domains", post(services::add_domain))
        .route("/domains/{id}", delete(services::delete_domain))
        .route("/services/{id}/volumes", post(services::add_volume))
        .route("/volumes/{id}", delete(services::delete_volume))
        .route("/services/{id}/database", patch(databases::update))
        .route(
            "/services/{id}/database/rotate-password",
            post(databases::rotate_password),
        )
        .route(
            "/services/{id}/backups",
            get(databases::list_backups).post(databases::create_backup),
        )
        .route(
            "/services/{id}/backups/schedule",
            get(databases::get_schedule).put(databases::set_schedule),
        )
        .route(
            "/backups/{id}",
            get(databases::get_backup).delete(databases::delete_backup),
        )
        .route("/backups/{id}/download", get(databases::download_backup))
        .route("/backups/{id}/restore", post(databases::restore_backup))
        .route("/secrets", get(secrets::list).post(secrets::create))
        .route("/secrets/generate-ssh-key", post(secrets::generate_ssh_key))
        .route(
            "/secrets/{id}",
            get(secrets::get)
                .patch(secrets::update)
                .delete(secrets::delete),
        )
        .route("/secrets/{id}/reveal", post(secrets::reveal))
        .route("/storage", get(storage::list).post(storage::create))
        .route(
            "/storage/{id}",
            get(storage::get)
                .patch(storage::update)
                .delete(storage::delete),
        )
        .route("/storage/{id}/test", post(storage::test))
        .route(
            "/instance/backups",
            get(instance::list).post(instance::create),
        )
        .route(
            "/instance/backups/schedule",
            get(instance::get_schedule).put(instance::set_schedule),
        )
        .route("/instance/backups/{id}", delete(instance::delete))
        .route("/instance/backups/{id}/download", get(instance::download))
        .route("/notify/catalog", get(notify::catalog))
        .route(
            "/notify/channels",
            get(notify::list_channels).post(notify::create_channel),
        )
        .route(
            "/notify/channels/{id}",
            get(notify::get_channel)
                .patch(notify::update_channel)
                .delete(notify::delete_channel),
        )
        .route("/notify/channels/{id}/test", post(notify::test_channel))
        .route(
            "/notify/rules",
            get(notify::list_rules).post(notify::create_rule),
        )
        .route(
            "/notify/rules/{id}",
            patch(notify::update_rule).delete(notify::delete_rule),
        )
        .route("/notify/deliveries", get(notify::deliveries))
        .route("/services/{id}/deployments", get(deployments::list))
        .route("/deployments/{id}", get(deployments::get))
        .route("/deployments/{id}/logs", get(deployments::logs))
        .route("/deployments/{id}/cancel", post(deployments::cancel))
        .route("/deployments/{id}/rollback", post(deployments::rollback))
        .route("/settings", get(settings::get).patch(settings::update))
        .route("/settings/token/rotate", post(settings::rotate_token))
        .route("/settings/proxy/restart", post(settings::restart_proxy))
        .route("/settings/prune", post(settings::prune))
        .route("/settings/version", get(settings::version))
        .route("/settings/update", post(settings::self_update))
        .route(
            "/settings/domain",
            get(domain::get).put(domain::set).delete(domain::remove),
        )
        .route("/settings/domain/check", post(domain::check))
        .route("/sources", get(sources::list))
        .route("/sources/github", post(sources::create_github_app))
        .route("/sources/github/connect", post(sources::connect_github_app))
        .route("/sources/github/token", post(sources::connect_github_token))
        .route("/sources/{id}", get(sources::get).delete(sources::delete))
        .route("/sources/{id}/check", post(sources::check))
        .route("/sources/{id}/repos", get(sources::repos))
        .route("/sources/{id}/branches", get(sources::branches))
        .route("/sources/{id}/detect", get(sources::detect))
        .route("/sources/{id}/dirs", get(sources::dirs))
        .route_layer(middleware::from_fn_with_state(state.clone(), auth));
    Router::new()
        .nest(API_PREFIX, api)
        .route("/github/{id}/start", get(github::start))
        .route("/github/{id}/callback", get(github::callback))
        .route("/github/{id}/setup", get(github::setup))
        .route("/github/{id}/webhook", post(github::webhook))
        .route("/healthz", get(|| async { "ok" }))
        .route("/.well-known/acme-challenge/{token}", get(domain::verify))
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
