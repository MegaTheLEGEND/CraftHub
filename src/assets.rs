//! The web UI (`ui/`), compiled into the binary.

use axum::{
    extract::Path,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use include_dir::{Dir, File, include_dir};

static UI: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/ui");

pub fn file(path: &str) -> Option<&'static File<'static>> {
    UI.get_file(path)
}

fn mime(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "woff2" => "font/woff2",
        "json" => "application/json",
        _ => "application/octet-stream",
    }
}

pub fn respond(path: &str) -> Response {
    let Some(f) = file(path) else { return StatusCode::NOT_FOUND.into_response() };
    // Code and markup are small and change with releases; fonts and pictures never do.
    let cache = if matches!(path.rsplit('.').next(), Some("woff2" | "png" | "svg")) { "public, max-age=604800" } else { "no-cache" };
    ([(header::CONTENT_TYPE, mime(path)), (header::CACHE_CONTROL, cache)], f.contents()).into_response()
}

pub async fn static_file(Path(path): Path<String>) -> Response {
    // `icons/<id>.png` are served through /api/apps/<id>/icon, which also covers apps without one.
    if path.contains("..") {
        return StatusCode::NOT_FOUND.into_response();
    }
    respond(&path)
}

pub async fn index() -> Response {
    respond("index.html")
}
