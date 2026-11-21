//! First-run setup: the checklist and where the flow is.

use super::Json;
use crate::error::{ApiResult, AppError};
use crate::onboarding;
use crate::state::AppState;
use axum::extract::State;
use common::api::UpdateOnboarding;
use common::onboarding::Onboarding;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Serializes read-modify-write of the stored state.
static WRITE: Mutex<()> = Mutex::const_new(());

pub async fn get(State(state): State<Arc<AppState>>) -> ApiResult<Json<Onboarding>> {
    let stored = onboarding::load(&state.db).await?;
    Ok(Json(stored.view(onboarding::facts(&state).await?)))
}

pub async fn update(
    State(state): State<Arc<AppState>>,
    Json(body): Json<UpdateOnboarding>,
) -> ApiResult<Json<Onboarding>> {
    let stored = {
        let _guard = WRITE.lock().await;
        let mut stored = onboarding::load(&state.db).await?;
        stored.apply(&body).map_err(AppError::BadRequest)?;
        onboarding::save(&state.db, &stored).await?;
        stored
    };
    Ok(Json(stored.view(onboarding::facts(&state).await?)))
}
