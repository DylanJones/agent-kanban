//! API error type rendered as RFC 7807 `application/problem+json`.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use utoipa::ToSchema;

/// RFC 7807 problem details.
#[derive(Debug, Serialize, ToSchema)]
pub struct Problem {
    /// Short machine-readable error code, e.g. `invalid_transition`.
    #[serde(rename = "type")]
    pub kind: String,
    pub title: String,
    pub status: u16,
    pub detail: Option<String>,
    /// For `invalid_transition`: the states the caller may move the issue to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allowed_transitions: Option<Vec<String>>,
}

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("unauthorized")]
    Unauthorized,
    #[error("forbidden: {0}")]
    Forbidden(String),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("invalid transition: {detail}")]
    InvalidTransition { detail: String, allowed: Vec<String> },
    #[error(transparent)]
    Db(#[from] sqlx::Error),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

pub type ApiResult<T> = Result<T, ApiError>;

impl ApiError {
    pub fn not_found(what: impl Into<String>) -> Self {
        ApiError::NotFound(what.into())
    }
    pub fn bad(msg: impl Into<String>) -> Self {
        ApiError::BadRequest(msg.into())
    }
    pub fn conflict(msg: impl Into<String>) -> Self {
        ApiError::Conflict(msg.into())
    }
    pub fn forbidden(msg: impl Into<String>) -> Self {
        ApiError::Forbidden(msg.into())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, kind, detail, allowed) = match &self {
            ApiError::NotFound(m) => (StatusCode::NOT_FOUND, "not_found", Some(m.clone()), None),
            ApiError::BadRequest(m) => (StatusCode::BAD_REQUEST, "bad_request", Some(m.clone()), None),
            ApiError::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized", Some("send `Authorization: Bearer <token>`".into()), None),
            ApiError::Forbidden(m) => (StatusCode::FORBIDDEN, "forbidden", Some(m.clone()), None),
            ApiError::Conflict(m) => (StatusCode::CONFLICT, "conflict", Some(m.clone()), None),
            ApiError::InvalidTransition { detail, allowed } => {
                (StatusCode::CONFLICT, "invalid_transition", Some(detail.clone()), Some(allowed.clone()))
            }
            ApiError::Db(sqlx::Error::RowNotFound) => (StatusCode::NOT_FOUND, "not_found", None, None),
            ApiError::Db(e) => {
                tracing::error!("database error: {e:?}");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal", Some(e.to_string()), None)
            }
            ApiError::Other(e) => {
                tracing::error!("internal error: {e:?}");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal", Some(format!("{e:#}")), None)
            }
        };
        let body = Problem {
            kind: kind.into(),
            title: status.canonical_reason().unwrap_or("error").into(),
            status: status.as_u16(),
            detail,
            allowed_transitions: allowed,
        };
        let mut resp = (status, Json(body)).into_response();
        resp.headers_mut().insert(axum::http::header::CONTENT_TYPE, "application/problem+json".parse().unwrap());
        resp
    }
}
