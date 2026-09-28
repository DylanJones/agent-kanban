use std::convert::Infallible;

use axum::Json;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{IntoResponse, Redirect, Response};
use futures::Stream;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::AppState;
use crate::auth::{self, Human};
use crate::db;
use crate::domain::Actor;
use crate::domain::models::ApiToken;
use crate::error::{ApiError, ApiResult};

#[derive(Debug, Serialize, ToSchema)]
pub struct Health {
    pub ok: bool,
    pub version: String,
}

/// Liveness check (no auth required).
#[utoipa::path(operation_id = "meta_health", get, path = "/api/health", tag = "meta", responses((status = 200, body = Health)))]
pub async fn health() -> Json<Health> {
    Json(Health { ok: true, version: env!("CARGO_PKG_VERSION").into() })
}

#[derive(Debug, Serialize, ToSchema)]
pub struct Me {
    /// `human` or `agent`.
    pub kind: String,
    pub name: String,
    pub run_id: Option<i64>,
    pub role: Option<String>,
    pub project: Option<String>,
    pub issue: Option<i64>,
}

/// Who am I? Describes the caller's identity and (for agents) the run's scope.
#[utoipa::path(operation_id = "meta_me", get, path = "/api/me", tag = "meta", responses((status = 200, body = Me)))]
pub async fn me(State(app): State<AppState>, actor: Actor) -> ApiResult<Json<Me>> {
    Ok(Json(match &actor {
        Actor::Agent { run_id, role, issue_id, project_id, .. } => {
            let project: Option<String> =
                sqlx::query_scalar("SELECT slug FROM projects WHERE id = ?").bind(project_id).fetch_optional(&app.db).await?;
            let issue: Option<i64> = match issue_id {
                Some(i) => sqlx::query_scalar("SELECT number FROM issues WHERE id = ?").bind(i).fetch_optional(&app.db).await?,
                None => None,
            };
            Me { kind: "agent".into(), name: actor.name(), run_id: Some(*run_id), role: Some(role.as_str().into()), project, issue }
        }
        _ => Me { kind: actor.kind().into(), name: actor.name(), run_id: None, role: None, project: None, issue: None },
    }))
}

#[derive(Debug, Deserialize)]
pub struct LoginQuery {
    t: String,
}

/// Browser login: `/login?t=<admin token>` sets a session cookie and redirects to the board.
pub async fn login(State(app): State<AppState>, Query(q): Query<LoginQuery>) -> Response {
    match auth::resolve(&app, Some(&q.t)).await {
        Ok(a) if a.is_human() => {
            let cookie = format!("{}={}; Path=/; HttpOnly; SameSite=Strict; Max-Age=31536000", auth::COOKIE, q.t);
            let mut r = Redirect::to("/").into_response();
            r.headers_mut().insert(header::SET_COOKIE, cookie.parse().unwrap());
            r
        }
        _ => (StatusCode::UNAUTHORIZED, "invalid login token").into_response(),
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct Settings {
    /// Global cap on simultaneously executing agent runs.
    pub max_concurrent_runs: i64,
    /// When false, no new runs are dispatched automatically (manual runs still work).
    pub scheduler_enabled: bool,
    /// Per-role run timeouts in minutes.
    #[schema(value_type = std::collections::HashMap<String, i64>)]
    pub run_timeouts_minutes: std::collections::BTreeMap<String, i64>,
    /// Default agent slug per role (projects may override).
    #[schema(value_type = std::collections::HashMap<String, String>)]
    pub default_role_agents: std::collections::BTreeMap<String, String>,
    /// Consecutive failed runs before an issue is marked stalled.
    pub max_failures: i64,
}

#[derive(Debug, Default, Deserialize, ToSchema)]
pub struct SettingsPatch {
    pub max_concurrent_runs: Option<i64>,
    pub scheduler_enabled: Option<bool>,
    #[schema(value_type = Option<std::collections::HashMap<String, i64>>)]
    pub run_timeouts_minutes: Option<std::collections::BTreeMap<String, i64>>,
    #[schema(value_type = Option<std::collections::HashMap<String, String>>)]
    pub default_role_agents: Option<std::collections::BTreeMap<String, String>>,
    pub max_failures: Option<i64>,
}

pub async fn load_settings(app: &AppState) -> Settings {
    Settings {
        max_concurrent_runs: db::get_setting(&app.db, "max_concurrent_runs").await.unwrap_or(3),
        scheduler_enabled: db::get_setting(&app.db, "scheduler_enabled").await.unwrap_or(false),
        run_timeouts_minutes: db::get_setting(&app.db, "run_timeouts_minutes").await.unwrap_or_default(),
        default_role_agents: db::get_setting(&app.db, "default_role_agents").await.unwrap_or_default(),
        max_failures: db::get_setting(&app.db, "max_failures").await.unwrap_or(3),
    }
}

/// Global settings.
#[utoipa::path(operation_id = "meta_get_settings", get, path = "/api/settings", tag = "settings", responses((status = 200, body = Settings)))]
pub async fn get_settings(State(app): State<AppState>, _a: Actor) -> Json<Settings> {
    Json(load_settings(&app).await)
}

/// Update global settings (concurrency limit, scheduler on/off, timeouts, default agents).
#[utoipa::path(operation_id = "meta_patch_settings", patch, path = "/api/settings", tag = "settings", request_body = SettingsPatch, responses((status = 200, body = Settings)))]
pub async fn patch_settings(State(app): State<AppState>, Human(_a): Human, Json(p): Json<SettingsPatch>) -> ApiResult<Json<Settings>> {
    if let Some(n) = p.max_concurrent_runs {
        if !(0..=64).contains(&n) {
            return Err(ApiError::bad("max_concurrent_runs must be between 0 and 64"));
        }
        db::set_setting(&app.db, "max_concurrent_runs", &n).await?;
    }
    if let Some(b) = p.scheduler_enabled {
        db::set_setting(&app.db, "scheduler_enabled", &b).await?;
    }
    if let Some(t) = &p.run_timeouts_minutes {
        let mut cur: std::collections::BTreeMap<String, i64> = db::get_setting(&app.db, "run_timeouts_minutes").await.unwrap_or_default();
        cur.extend(t.clone());
        db::set_setting(&app.db, "run_timeouts_minutes", &cur).await?;
    }
    if let Some(r) = &p.default_role_agents {
        let mut cur: std::collections::BTreeMap<String, String> = db::get_setting(&app.db, "default_role_agents").await.unwrap_or_default();
        cur.extend(r.clone());
        db::set_setting(&app.db, "default_role_agents", &cur).await?;
    }
    if let Some(n) = p.max_failures {
        db::set_setting(&app.db, "max_failures", &n.max(1)).await?;
    }
    app.bus.emit("settings.updated", None, None, None, None);
    Ok(Json(load_settings(&app).await))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct NewToken {
    pub name: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct CreatedToken {
    pub id: i64,
    pub name: String,
    /// Shown once; use as `Authorization: Bearer <token>`.
    pub token: String,
}

/// List human API tokens.
#[utoipa::path(operation_id = "meta_list_tokens", get, path = "/api/tokens", tag = "settings", responses((status = 200, body = Vec<ApiToken>)))]
pub async fn list_tokens(State(app): State<AppState>, Human(_a): Human) -> ApiResult<Json<Vec<ApiToken>>> {
    Ok(Json(sqlx::query_as::<_, ApiToken>("SELECT * FROM api_tokens WHERE revoked_at IS NULL ORDER BY id").fetch_all(&app.db).await?))
}

/// Create a human API token for scripts.
#[utoipa::path(operation_id = "meta_create_token", post, path = "/api/tokens", tag = "settings", request_body = NewToken, responses((status = 201, body = CreatedToken)))]
pub async fn create_token(
    State(app): State<AppState>,
    Human(_a): Human,
    Json(req): Json<NewToken>,
) -> ApiResult<(StatusCode, Json<CreatedToken>)> {
    let (id, token) = create_api_token(&app.db, &req.name).await?;
    Ok((StatusCode::CREATED, Json(CreatedToken { id, name: req.name, token })))
}

pub async fn create_api_token(db: &db::Db, name: &str) -> sqlx::Result<(i64, String)> {
    let token = format!("akt_{}", auth::random_token());
    let id = sqlx::query_scalar("INSERT INTO api_tokens(name, token_hash, created_at) VALUES (?, ?, ?) RETURNING id")
        .bind(name)
        .bind(auth::hash_token(&token))
        .bind(db::now())
        .fetch_one(db)
        .await?;
    Ok((id, token))
}

/// Revoke a human API token.
#[utoipa::path(operation_id = "meta_revoke_token", delete, path = "/api/tokens/{id}", tag = "settings", params(("id" = i64, Path)), responses((status = 204)))]
pub async fn revoke_token(
    State(app): State<AppState>,
    Human(_a): Human,
    axum::extract::Path(id): axum::extract::Path<i64>,
) -> ApiResult<StatusCode> {
    sqlx::query("UPDATE api_tokens SET revoked_at = ? WHERE id = ?").bind(db::now()).bind(id).execute(&app.db).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Server-sent events for UI live updates. Each event is a `DomainEvent` JSON object.
#[utoipa::path(operation_id = "meta_events_stream", get, path = "/api/events/stream", tag = "meta", responses((status = 200, description = "text/event-stream of DomainEvent", content_type = "text/event-stream")))]
pub async fn events_stream(State(app): State<AppState>, _a: Actor) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>>> {
    let mut rx = app.bus.subscribe();
    let stream = async_stream::stream! {
        loop {
            match rx.recv().await {
                Ok(ev) => yield Ok(SseEvent::default().event("domain").json_data(&ev).unwrap()),
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => yield Ok(SseEvent::default().event("resync").data("{}")),
                Err(_) => break,
            }
        }
    };
    Sse::new(stream).keep_alive(KeepAlive::default())
}

/// The agent guide (markdown), personalised with the caller's API base URL, project, issue and token.
#[utoipa::path(operation_id = "meta_agent_guide", get, path = "/api/agent-guide", tag = "meta", responses((status = 200, description = "Markdown", content_type = "text/markdown")))]
pub async fn agent_guide(State(app): State<AppState>, actor: Actor, headers: HeaderMap) -> ApiResult<Response> {
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("$AKB_AUTH")
        .to_string();
    let (project, issue, role) = match &actor {
        Actor::Agent { project_id, issue_id, role, .. } => {
            let p: Option<String> =
                sqlx::query_scalar("SELECT slug FROM projects WHERE id = ?").bind(project_id).fetch_optional(&app.db).await?;
            let i: Option<i64> = match issue_id {
                Some(i) => sqlx::query_scalar("SELECT number FROM issues WHERE id = ?").bind(i).fetch_optional(&app.db).await?,
                None => None,
            };
            (p, i, Some(role.as_str().to_string()))
        }
        _ => (None, None, None),
    };
    let md = crate::orchestrator::prompt::render_guide(&app.config.api_url(), project.as_deref(), issue, role.as_deref(), &token);
    Ok(([(header::CONTENT_TYPE, "text/markdown; charset=utf-8")], md).into_response())
}
