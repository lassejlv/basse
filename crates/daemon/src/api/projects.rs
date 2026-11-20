//! Projects and environments.

use super::{Json, no_content};
use crate::db::new_id;
use crate::error::{ApiResult, AppError, conflict};
use crate::services;
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use chrono::Utc;
use common::api::{CreateEnvironment, CreateProject};
use common::*;
use std::sync::Arc;

pub async fn list(State(state): State<Arc<AppState>>) -> ApiResult<Json<Vec<Project>>> {
    Ok(Json(state.db.projects()?))
}

pub async fn create(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateProject>,
) -> ApiResult<Json<Project>> {
    validate_name(&body.name).map_err(AppError::BadRequest)?;
    if state.db.project_name_taken(&body.name)? {
        conflict!("a project named '{}' already exists", body.name);
    }
    let project = Project {
        id: new_id(),
        name: body.name,
        description: body
            .description
            .map(|d| d.trim().to_string())
            .filter(|d| !d.is_empty()),
        created_at: Utc::now(),
    };
    let production = Environment {
        id: new_id(),
        project_id: project.id.clone(),
        name: "production".into(),
        created_at: Utc::now(),
    };
    state.db.insert_project(&project, &production)?;
    state.project_activity(
        &project.id,
        ActivityLevel::Info,
        format!("Project {} created", project.name),
    );
    Ok(Json(project))
}

pub async fn delete(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let project = state
        .db
        .project(&id)?
        .ok_or_else(|| AppError::not_found("project", &id))?;
    for env in state
        .db
        .environments()?
        .iter()
        .filter(|e| e.project_id == id)
    {
        services::delete_environment_services(&state, env).await?;
    }
    state.db.delete_project(&id)?;
    state.project_activity(
        &id,
        ActivityLevel::Info,
        format!("Project {} deleted", project.name),
    );
    Ok(no_content())
}

pub async fn create_environment(
    State(state): State<Arc<AppState>>,
    Path(project_id): Path<String>,
    Json(body): Json<CreateEnvironment>,
) -> ApiResult<Json<Environment>> {
    let project = state
        .db
        .project(&project_id)?
        .ok_or_else(|| AppError::not_found("project", &project_id))?;
    validate_name(&body.name).map_err(AppError::BadRequest)?;
    if state.db.environment_name_taken(&project.id, &body.name)? {
        conflict!(
            "project '{}' already has an environment named '{}'",
            project.name,
            body.name
        );
    }
    let env = Environment {
        id: new_id(),
        project_id: project.id.clone(),
        name: body.name,
        created_at: Utc::now(),
    };
    state.db.insert_environment(&env)?;
    state.project_activity(
        &project.id,
        ActivityLevel::Info,
        format!("Environment {} created in {}", env.name, project.name),
    );
    Ok(Json(env))
}

pub async fn delete_environment(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let env = state
        .db
        .environment(&id)?
        .ok_or_else(|| AppError::not_found("environment", &id))?;
    services::delete_environment_services(&state, &env).await?;
    state.db.delete_environment(&id)?;
    state.project_activity(
        &env.project_id,
        ActivityLevel::Info,
        format!("Environment {} deleted", env.name),
    );
    Ok(no_content())
}
