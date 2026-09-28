//! Rebuild and restart the server from the app (issue #12). Admin only; agents can't trigger it
//! (see `auth::Human`).

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::json;
use utoipa::ToSchema;

use crate::AppState;
use crate::auth::Human;
use crate::deploy::{self, BuildStatus, RestartMode};
use crate::domain::Actor;
use crate::domain::models::Job;
use crate::error::{ApiError, ApiResult};
use crate::jobs;

/// How far the running server is behind the checkout it was built from, and whether a rebuild is
/// in progress.
#[utoipa::path(operation_id = "deploy_status", get, path = "/api/build-status", tag = "deploy", responses((status = 200, body = BuildStatus)))]
pub async fn build_status(State(app): State<AppState>, _a: Actor) -> Json<BuildStatus> {
    Json(deploy::status(&app).await)
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct RebuildRequest {
    pub mode: RestartMode,
}

/// Rebuild the web UI and server from the checkout, then restart. Runs as a background job
/// (stream its log via `GET /api/jobs/{id}` or `kind=server.build`); the restart only happens if
/// the build succeeds. `mode: "now"` interrupts active agent runs (they resume after the
/// restart); `mode: "drain"` stops the scheduler from starting new runs and restarts once the
/// active ones finish.
#[utoipa::path(operation_id = "deploy_rebuild", post, path = "/api/build-status/rebuild", tag = "deploy", request_body = RebuildRequest, responses((status = 202, body = Job)))]
pub async fn rebuild(State(app): State<AppState>, Human(_a): Human, Json(req): Json<RebuildRequest>) -> ApiResult<(StatusCode, Json<Job>)> {
    if !deploy::try_begin_deploy(&app) {
        return Err(ApiError::conflict("a rebuild is already in progress"));
    }
    if req.mode == RestartMode::Drain {
        deploy::begin_drain(&app).await;
    }
    let mode = match req.mode {
        RestartMode::Now => "now",
        RestartMode::Drain => "drain",
    };
    let id = jobs::enqueue(&app, "server.build", None, json!({"mode": mode})).await;
    let job = sqlx::query_as::<_, Job>("SELECT * FROM jobs WHERE id = ?").bind(id).fetch_one(&app.db).await?;
    Ok((StatusCode::ACCEPTED, Json(job)))
}
