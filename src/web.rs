//! Serves the built React app (embedded at compile time) with SPA fallback.

use axum::http::{StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "web/dist/"]
#[allow_missing = true]
struct Assets;

pub async fn static_handler(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    if path.starts_with("api/") {
        return (StatusCode::NOT_FOUND, "no such API route").into_response();
    }
    let file = if path.is_empty() { "index.html" } else { path };
    if let Some(content) = Assets::get(file) {
        let mime = mime_guess::from_path(file).first_or_octet_stream();
        let cache = if file.starts_with("assets/") { "public, max-age=31536000, immutable" } else { "no-cache" };
        return ([(header::CONTENT_TYPE, mime.as_ref().to_string()), (header::CACHE_CONTROL, cache.to_string())], content.data)
            .into_response();
    }
    match Assets::get("index.html") {
        Some(index) => ([(header::CONTENT_TYPE, "text/html".to_string()), (header::CACHE_CONTROL, "no-cache".to_string())], index.data).into_response(),
        None => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "text/html")],
            "<h1>agent-kanban</h1><p>The web UI isn't built. Run <code>npm --prefix web install && npm --prefix web run build</code> and rebuild, or use the Vite dev server (<code>npm --prefix web run dev</code>). API docs: <a href=\"/api/docs\">/api/docs</a></p>",
        )
            .into_response(),
    }
}
