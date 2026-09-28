//! Image attachment upload/download, for inline images in issue and comment markdown.

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};

use crate::AppState;
use crate::domain::Actor;
use crate::error::ApiResult;
use crate::services::{self, attachments};

/// Upload an image to attach to an issue or comment (PNG, JPEG, GIF or WebP; 10 MiB max).
///
/// Returns a same-origin `url` and a ready-to-paste `markdown` snippet (`![](url)`). Works before
/// the issue it will be attached to exists — upload first, then include the markdown in the body.
#[utoipa::path(operation_id = "attachments_upload", post, path = "/api/projects/{p}/attachments", tag = "issues",
    params(("p" = String, Path, description = "Project slug")),
    request_body(content = Vec<u8>, description = "Raw image bytes"),
    responses((status = 201, body = attachments::Attachment)))]
pub async fn upload(
    State(app): State<AppState>,
    actor: Actor,
    Path(p): Path<String>,
    body: Bytes,
) -> ApiResult<(StatusCode, Json<attachments::Attachment>)> {
    let project = services::project(&app.db, &p).await?;
    services::ensure_project_access(&actor, &project)?;
    let a = attachments::save(&app, &project, &actor, body.to_vec()).await?;
    Ok((StatusCode::CREATED, Json(a)))
}

/// Download an attached image. Requires the same login as everything else in the board.
#[utoipa::path(operation_id = "attachments_download", get, path = "/api/projects/{p}/attachments/{filename}", tag = "issues",
    params(("p" = String, Path, description = "Project slug"), ("filename" = String, Path)),
    responses((status = 200, description = "Image bytes", content_type = "application/octet-stream")))]
pub async fn download(State(app): State<AppState>, _actor: Actor, Path((p, filename)): Path<(String, String)>) -> ApiResult<Response> {
    let project = services::project(&app.db, &p).await?;
    let (content_type, bytes) = attachments::load(&app, &project, &filename).await?;
    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, content_type.parse().unwrap());
    headers.insert(header::CACHE_CONTROL, "private, max-age=31536000, immutable".parse().unwrap());
    headers.insert(header::X_CONTENT_TYPE_OPTIONS, "nosniff".parse().unwrap());
    Ok((headers, bytes).into_response())
}
