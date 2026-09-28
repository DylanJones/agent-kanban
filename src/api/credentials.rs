//! Credentials agents need inside containers. Values are write-only: the API reports whether
//! something is configured, never the secret itself.

use std::time::Duration;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::AppState;
use crate::auth::Human;
use crate::domain::Actor;
use crate::domain::models::{AgentDefinition, Project};
use crate::error::{ApiError, ApiResult};
use crate::orchestrator::probe::{self, AgentTestResult};

#[derive(Debug, Serialize, ToSchema)]
pub struct Credentials {
    /// A long-lived Claude token for containers is configured.
    pub claude_token: bool,
    /// `~/.codex/auth.json` exists (mounted into Codex containers).
    pub codex_auth: bool,
    /// Projects that run agents in containers.
    pub container_projects: Vec<String>,
}

async fn container_projects(app: &AppState) -> sqlx::Result<Vec<Project>> {
    sqlx::query_as::<_, Project>("SELECT * FROM projects WHERE container_enabled = 1 ORDER BY slug").fetch_all(&app.db).await
}

/// Which container credentials are configured.
#[utoipa::path(operation_id = "credentials_get", get, path = "/api/credentials", tag = "agents", responses((status = 200, body = Credentials)))]
pub async fn get(State(app): State<AppState>, _a: Actor) -> ApiResult<Json<Credentials>> {
    let secrets = app.config.current_secrets();
    Ok(Json(Credentials {
        claude_token: secrets.claude_code_oauth_token.as_deref().is_some_and(|t| !t.trim().is_empty()),
        codex_auth: dirs::home_dir().is_some_and(|h| h.join(".codex/auth.json").exists()),
        container_projects: container_projects(&app).await?.into_iter().map(|p| p.slug).collect(),
    }))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct ClaudeToken {
    /// The token printed by `claude setup-token` (starts with `sk-ant-oat`).
    pub token: String,
    /// Check the token by running Claude in a project container before saving (default true).
    #[serde(default = "yes")]
    pub verify: bool,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SavedCredential {
    pub saved: bool,
    /// The verification run (if one was done).
    pub test: Option<AgentTestResult>,
    /// Project whose container was used for verification.
    pub tested_in: Option<String>,
}

/// Save Claude's long-lived token for container runs, after checking it works there.
#[utoipa::path(operation_id = "credentials_put_claude", put, path = "/api/credentials/claude-token", tag = "agents", request_body = ClaudeToken,
    responses((status = 200, body = SavedCredential), (status = 422, body = SavedCredential)))]
pub async fn put_claude(State(app): State<AppState>, Human(_a): Human, Json(req): Json<ClaudeToken>) -> ApiResult<(StatusCode, Json<SavedCredential>)> {
    let token = req.token.trim().to_string();
    if token.is_empty() || token.chars().any(char::is_whitespace) {
        return Err(ApiError::bad("paste just the token (one line, no spaces)"));
    }
    if !token.starts_with("sk-ant-") {
        return Err(ApiError::bad("that doesn't look like a Claude token (they start with `sk-ant-`); copy the line `claude setup-token` printed"));
    }
    let (mut test, mut tested_in) = (None, None);
    if req.verify {
        let claude = sqlx::query_as::<_, AgentDefinition>("SELECT * FROM agent_definitions WHERE harness = 'claude' ORDER BY id LIMIT 1")
            .fetch_optional(&app.db)
            .await?
            .ok_or_else(|| ApiError::bad("no Claude agent is defined"))?;
        if let Some(project) = container_projects(&app).await?.into_iter().find(|p| p.container_image.is_some()) {
            let r = probe::session_check(
                &app,
                &claude,
                Some(&project),
                vec![("CLAUDE_CODE_OAUTH_TOKEN".into(), token.clone())],
                Some("Reply with exactly: OK"),
                Duration::from_secs(180),
            )
            .await?;
            tested_in = Some(project.slug.clone());
            if !r.ok {
                return Ok((StatusCode::UNPROCESSABLE_ENTITY, Json(SavedCredential { saved: false, test: Some(r), tested_in })));
            }
            test = Some(r);
        }
    }
    let mut secrets = app.config.current_secrets();
    secrets.claude_code_oauth_token = Some(token);
    crate::config::write_secrets(&app.config.data_dir.join("secrets.toml"), &secrets)?;
    // Anything paused because Claude couldn't log in may run now.
    let _ = sqlx::query("UPDATE agent_definitions SET needs_auth = 0 WHERE harness = 'claude'").execute(&app.db).await;
    let groups: Vec<String> = sqlx::query_scalar("SELECT DISTINCT limit_group FROM agent_definitions WHERE harness = 'claude'").fetch_all(&app.db).await?;
    for g in groups {
        let kind: Option<String> = sqlx::query_scalar("SELECT pause_kind FROM limit_groups WHERE name = ? AND paused = 1").bind(&g).fetch_optional(&app.db).await?.flatten();
        if kind.as_deref() == Some("auth") {
            crate::orchestrator::limits::resume(&app, &g).await?;
        }
    }
    app.bus.emit("agents.updated", None, None, None, None);
    Ok((StatusCode::OK, Json(SavedCredential { saved: true, test, tested_in })))
}

/// Remove Claude's container token.
#[utoipa::path(operation_id = "credentials_delete_claude", delete, path = "/api/credentials/claude-token", tag = "agents", responses((status = 204)))]
pub async fn delete_claude(State(app): State<AppState>, Human(_a): Human) -> ApiResult<StatusCode> {
    let mut secrets = app.config.current_secrets();
    secrets.claude_code_oauth_token = None;
    crate::config::write_secrets(&app.config.data_dir.join("secrets.toml"), &secrets)?;
    app.bus.emit("agents.updated", None, None, None, None);
    Ok(StatusCode::NO_CONTENT)
}
