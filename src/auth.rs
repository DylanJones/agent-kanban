//! Request authentication: resolves the caller to an [`Actor`].
//!
//! * Humans: the admin token (login cookie or bearer) or a named API token.
//! * Agents: a per-run bearer token `akr_…`, valid only while the run is active (plus a grace period).

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use sha2::{Digest, Sha256};

use crate::AppState;
use crate::db;
use crate::domain::{Actor, Role};
use crate::error::ApiError;

pub const COOKIE: &str = "akb_session";

pub fn random_token() -> String {
    let mut buf = [0u8; 24];
    getrandom::fill(&mut buf).expect("os rng");
    hex::encode(buf)
}

pub fn hash_token(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

fn extract_token(parts: &Parts) -> Option<String> {
    if let Some(h) = parts.headers.get(axum::http::header::AUTHORIZATION).and_then(|v| v.to_str().ok())
        && let Some(t) = h.strip_prefix("Bearer ").or_else(|| h.strip_prefix("bearer "))
    {
        return Some(t.trim().to_string());
    }
    let cookies = parts.headers.get_all(axum::http::header::COOKIE);
    for c in cookies {
        if let Ok(s) = c.to_str() {
            for kv in s.split(';') {
                if let Some(v) = kv.trim().strip_prefix(&format!("{COOKIE}=")) {
                    return Some(v.to_string());
                }
            }
        }
    }
    // SSE via EventSource cannot set headers; allow ?token= on stream endpoints.
    if parts.uri.path().ends_with("/stream")
        && let Some(q) = parts.uri.query()
    {
        for kv in q.split('&') {
            if let Some(v) = kv.strip_prefix("token=") {
                return Some(v.to_string());
            }
        }
    }
    None
}

pub async fn resolve(app: &AppState, token: Option<&str>) -> Result<Actor, ApiError> {
    let Some(token) = token else {
        return if app.config.no_auth { Ok(Actor::human(human_name())) } else { Err(ApiError::Unauthorized) };
    };
    if token.starts_with("akr_") {
        let hash = hash_token(token);
        let row: Option<(i64, String, Option<i64>, i64, String, String, Option<String>)> = sqlx::query_as(
            "SELECT r.id, r.role, r.issue_id, r.project_id, a.slug, r.status, r.token_expires_at
               FROM agent_runs r JOIN agent_definitions a ON a.id = r.agent_definition_id
              WHERE r.token_hash = ?",
        )
        .bind(&hash)
        .fetch_optional(&app.db)
        .await?;
        let Some((run_id, role, issue_id, project_id, agent, status, expires)) = row else {
            return Err(ApiError::Unauthorized);
        };
        let active = matches!(status.as_str(), "queued" | "preparing" | "running");
        let in_grace = expires.as_deref().and_then(db::parse_time).is_some_and(|t| t > chrono::Utc::now());
        if !active && !in_grace {
            return Err(ApiError::forbidden("this run has ended; its token is no longer valid"));
        }
        let role = Role::parse(&role).ok_or(ApiError::Unauthorized)?;
        return Ok(Actor::Agent { run_id, role, issue_id, project_id, agent_name: agent });
    }
    if token == app.config.secrets.admin_token {
        return Ok(Actor::human(human_name()));
    }
    let hash = hash_token(token);
    let name: Option<String> = sqlx::query_scalar("SELECT name FROM api_tokens WHERE token_hash = ? AND revoked_at IS NULL")
        .bind(&hash)
        .fetch_optional(&app.db)
        .await?;
    match name {
        Some(n) => {
            let _ = sqlx::query("UPDATE api_tokens SET last_used_at = ? WHERE token_hash = ?")
                .bind(db::now())
                .bind(&hash)
                .execute(&app.db)
                .await;
            Ok(Actor::human(n))
        }
        None => Err(ApiError::Unauthorized),
    }
}

pub fn human_name() -> String {
    std::env::var("AKB_HUMAN_NAME").or_else(|_| std::env::var("USER")).unwrap_or_else(|_| "human".into())
}

impl FromRequestParts<AppState> for Actor {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, app: &AppState) -> Result<Self, Self::Rejection> {
        let token = extract_token(parts);
        resolve(app, token.as_deref()).await
    }
}

/// Extractor that only admits humans.
pub struct Human(pub Actor);

impl FromRequestParts<AppState> for Human {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, app: &AppState) -> Result<Self, Self::Rejection> {
        let actor = Actor::from_request_parts(parts, app).await?;
        if actor.is_human() { Ok(Human(actor)) } else { Err(ApiError::forbidden("humans only")) }
    }
}
