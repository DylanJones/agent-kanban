//! Image attachment storage: uploaded images live under `<data_dir>/attachments/<project_id>/<filename>`,
//! behind the same authentication as everything else. Only bytes that sniff as a supported image
//! format are ever written; the filename is always a server-generated opaque token, never the
//! caller's.

use anyhow::Context;
use serde::Serialize;
use utoipa::ToSchema;

use crate::AppState;
use crate::domain::Actor;
use crate::domain::models::Project;
use crate::error::{ApiError, ApiResult};

/// Decoded payload size cap (base64 MCP uploads are capped by the same limit after decoding).
pub const MAX_BYTES: usize = 10 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct Attachment {
    pub id: i64,
    pub filename: String,
    pub content_type: String,
    pub byte_size: i64,
    /// Same-origin URL; requires the caller's normal login.
    pub url: String,
    /// Ready-to-paste markdown, e.g. `![](url)`.
    pub markdown: String,
}

/// Identify a supported raster format from its magic bytes and pick a file extension.
/// Trusting sniffed bytes rather than a client-supplied filename/MIME keeps the allowlist honest.
fn sniff(bytes: &[u8]) -> Option<(&'static str, &'static str)> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(("image/png", "png"))
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        Some(("image/jpeg", "jpg"))
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some(("image/gif", "gif"))
    } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some(("image/webp", "webp"))
    } else {
        None
    }
}

pub async fn save(app: &AppState, project: &Project, actor: &Actor, bytes: Vec<u8>) -> ApiResult<Attachment> {
    if bytes.is_empty() {
        return Err(ApiError::bad("empty upload"));
    }
    if bytes.len() > MAX_BYTES {
        return Err(ApiError::bad(format!("image exceeds the {} MiB limit", MAX_BYTES / (1024 * 1024))));
    }
    let Some((content_type, ext)) = sniff(&bytes) else {
        return Err(ApiError::bad("unsupported image type; use PNG, JPEG, GIF or WebP"));
    };
    let filename = format!("{}.{ext}", crate::auth::random_token());
    let dir = app.config.attachments_dir().join(project.id.to_string());
    tokio::fs::create_dir_all(&dir).await.context("creating attachments directory")?;
    tokio::fs::write(dir.join(&filename), &bytes).await.context("writing attachment")?;
    let byte_size = bytes.len() as i64;
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO attachments(project_id, filename, content_type, byte_size, created_by_kind, created_by_name, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(project.id)
    .bind(&filename)
    .bind(content_type)
    .bind(byte_size)
    .bind(actor.kind())
    .bind(actor.name())
    .bind(crate::db::now())
    .fetch_one(&app.db)
    .await?;
    let url = format!("/api/projects/{}/attachments/{filename}", project.slug);
    Ok(Attachment { id, filename, content_type: content_type.into(), byte_size, markdown: format!("![]({url})"), url })
}

/// Load an attachment's bytes for download. Rejects anything not recorded for this project,
/// which also rules out path traversal since only exact recorded filenames match.
pub async fn load(app: &AppState, project: &Project, filename: &str) -> ApiResult<(String, Vec<u8>)> {
    let content_type: Option<String> = sqlx::query_scalar("SELECT content_type FROM attachments WHERE project_id = ? AND filename = ?")
        .bind(project.id)
        .bind(filename)
        .fetch_optional(&app.db)
        .await?;
    let Some(content_type) = content_type else { return Err(ApiError::not_found("attachment")) };
    let path = app.config.attachments_dir().join(project.id.to_string()).join(filename);
    let bytes = tokio::fs::read(&path).await.map_err(|_| ApiError::not_found("attachment"))?;
    Ok((content_type, bytes))
}
