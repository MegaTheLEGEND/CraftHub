//! Serves the installed web builds as static sites under /apps/{id}/.

use crate::auth::SessionUser;
use crate::config::is_safe_name;
use crate::state::AppState;
use axum::{
    extract::{Path, Request, State},
    http::{header, HeaderValue, StatusCode, Uri},
    response::{Html, IntoResponse, Redirect, Response},
};
use std::path::PathBuf;
use tower::ServiceExt;
use tower_http::services::ServeDir;

fn not_found(msg: &str) -> Response {
    (
        StatusCode::NOT_FOUND,
        Html(format!(
            r#"<!doctype html><meta charset="utf-8"><title>Not available</title>
<body style="font:16px system-ui;background:#0f1115;color:#e6e8ee;display:grid;place-items:center;min-height:100vh;margin:0">
<main style="max-width:30rem;padding:2rem"><h1>Not available</h1><p>{msg}</p><p><a style="color:#5aa2ff" href="/">Back to CraftHub</a></p></main>"#
        )),
    )
        .into_response()
}

pub async fn redirect_to_slash(Path(id): Path<String>) -> Redirect {
    Redirect::permanent(&format!("/apps/{id}/"))
}

pub async fn serve_app_root(State(st): State<AppState>, Path(id): Path<String>, req: Request) -> Response {
    serve_app(st, id, req).await
}

pub async fn serve_app_path(
    State(st): State<AppState>,
    Path((id, _rest)): Path<(String, String)>,
    req: Request,
) -> Response {
    serve_app(st, id, req).await
}

async fn serve_app(st: AppState, id: String, req: Request) -> Response {
    if st.app_def(&id).is_none() {
        return not_found("Unknown app.");
    }
    let rec = st.store.record(&id).await;
    let Some(version) = rec.active else {
        return not_found("This app has no installed web build yet. An administrator can install one from the launcher.");
    };
    let dir = st.apps_dir().join(&id).join(&version);
    let prefix = format!("/apps/{id}");
    serve_dir(dir, &prefix, req).await
}

/// Preview any installed version (admins only), e.g. before rolling back.
pub async fn serve_version_root(
    State(st): State<AppState>,
    Path((id, version)): Path<(String, String)>,
    req: Request,
) -> Response {
    serve_version(st, id, version, req).await
}

pub async fn serve_version_path(
    State(st): State<AppState>,
    Path((id, version, _rest)): Path<(String, String, String)>,
    req: Request,
) -> Response {
    serve_version(st, id, version, req).await
}

async fn serve_version(st: AppState, id: String, version: String, req: Request) -> Response {
    let is_admin = req.extensions().get::<SessionUser>().map(|u| u.admin).unwrap_or(false);
    if !is_admin {
        return (StatusCode::FORBIDDEN, "admins only").into_response();
    }
    if st.app_def(&id).is_none() || !is_safe_name(&version) {
        return not_found("Unknown app or version.");
    }
    let rec = st.store.record(&id).await;
    if !rec.installed.iter().any(|i| i.version == version) {
        return not_found("That version is not installed.");
    }
    let dir = st.apps_dir().join(&id).join(&version);
    let prefix = format!("/versions/{id}/{version}");
    serve_dir(dir, &prefix, req).await
}

async fn serve_dir(dir: PathBuf, prefix: &str, req: Request) -> Response {
    if !dir.is_dir() {
        return not_found("The files for this version are missing on disk.");
    }
    // Re-root the request at the site folder, keeping the original percent-encoding.
    let (mut parts, body) = req.into_parts();
    let pq = parts.uri.path_and_query().map(|p| p.as_str()).unwrap_or("/");
    let rest = pq.strip_prefix(prefix).unwrap_or("/");
    let rest = if rest.is_empty() || rest.starts_with('?') { format!("/{rest}") } else { rest.to_string() };
    let path_only = rest.split('?').next().unwrap_or("/").to_string();
    match rest.parse::<Uri>() {
        Ok(u) => parts.uri = u,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    }
    let req = Request::from_parts(parts, body);

    let svc = ServeDir::new(&dir)
        .precompressed_br()
        .precompressed_gzip()
        .append_index_html_on_directories(true);
    let mut res = match svc.oneshot(req).await {
        Ok(r) => r.into_response(),
        Err(never) => match never {},
    };

    if res.status().is_success() || res.status() == StatusCode::NOT_MODIFIED {
        let last = path_only.rsplit('/').next().unwrap_or("");
        let ext = last.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
        let cache = match ext.as_deref() {
            None => "no-cache",
            Some("html") => "no-cache",
            // Trunk emits content-hashed names like photocraft-web-<hash>_bg.wasm
            Some("js") | Some("wasm") | Some("css") if has_content_hash(last) => "public, max-age=31536000, immutable",
            _ => "public, max-age=3600",
        };
        res.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    }
    res
}

/// True for names carrying a bundler content hash (Trunk: `app-<16 hex>_bg.wasm`).
/// Only those may be cached "forever": the same URL under another version must differ.
fn has_content_hash(file: &str) -> bool {
    file.split(['-', '_', '.'])
        .any(|t| t.len() >= 8 && t.chars().all(|c| c.is_ascii_hexdigit()) && t.chars().any(|c| c.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_hashed_assets() {
        assert!(has_content_hash("photocraft-web-a1b2c3d4e5f60718_bg.wasm"));
        assert!(has_content_hash("app-0123456789abcdef.js"));
        assert!(!has_content_hash("photocraft-web.js"));
        assert!(!has_content_hash("service-worker.js"));
        assert!(!has_content_hash("index.css"));
    }
}
