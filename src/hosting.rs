//! Serves the installed web builds as static sites under /apps/{id}/.

use crate::auth::SessionUser;
use crate::config::is_safe_name;
use crate::state::AppState;
use axum::{
    extract::{Path, Request, State},
    http::{header, HeaderValue, Method, StatusCode, Uri},
    response::{Html, IntoResponse, Redirect, Response},
};
use std::path::PathBuf;
use tower::ServiceExt;
use tower_http::services::ServeDir;

fn not_found(msg: &str) -> Response {
    (
        StatusCode::NOT_FOUND,
        Html(format!(
            r#"<!doctype html><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Not available</title><style>:root{{color-scheme:light dark;--bg:#f2f1ee;--ink:#101014;--muted:rgba(16,16,20,.62);--line:rgba(16,16,20,.4);--accent:#1659c4;--inv:#101014;--invfg:#f2f1ee}}
@media(prefers-color-scheme:dark){{:root{{--bg:#121316;--ink:#f2f1ee;--muted:rgba(242,241,238,.6);--line:rgba(242,241,238,.4);--accent:#74aaff;--inv:#fff;--invfg:#0b0b0c}}}}
body{{font:14px/1.5 Inter,system-ui,sans-serif;background:var(--bg);color:var(--ink);display:grid;place-items:center;min-height:100vh;margin:0}}
main{{max-width:30rem;padding:2rem;border:1px solid var(--line)}}h1{{font:600 22px system-ui;margin:0 0 .8rem}}
a{{color:var(--accent)}}.tag{{display:inline-block;background:var(--inv);color:var(--invfg);font:700 11px ui-monospace,Menlo,monospace;letter-spacing:.15em;text-transform:uppercase;padding:3px 8px;margin-bottom:1rem}}</style>
<main><span class="tag">{brand}</span><h1>Not available</h1><p>{msg}</p><p><a href="/">Back to {brand}</a></p></main>"#,
            brand = crate::brand::PRODUCT
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
    serve_dir(dir, &prefix, &icon_href(&id), req).await
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
    serve_dir(dir, &prefix, &icon_href(&id), req).await
}

async fn serve_dir(dir: PathBuf, prefix: &str, icon: &str, req: Request) -> Response {
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

    // Web builds that declare no favicon get the app's launcher icon, so the tab is not blank.
    if matches!(*req.method(), Method::GET | Method::HEAD) {
        if let Some(res) = html_with_icon(&dir, &path_only, icon).await {
            return res;
        }
    }

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

/// Where the launcher serves an app's icon (the same picture the cards show).
fn icon_href(id: &str) -> String {
    format!("/api/apps/{id}/icon")
}

/// The largest page we will read into memory to add an icon to.
const MAX_HTML_BYTES: u64 = 4 << 20;

/// True when some `<link>` in `html` already declares an icon (`rel="icon"`, `rel="shortcut icon"`, ...).
/// `apple-touch-icon` alone does not count: browser tabs do not use it.
fn declares_icon(html: &str) -> bool {
    let lower = html.to_ascii_lowercase();
    let mut rest = lower.as_str();
    while let Some(i) = rest.find("<link") {
        let tag = &rest[i..];
        let end = tag.find('>').unwrap_or(tag.len());
        let tag = &tag[..end];
        if let Some(r) = tag.find("rel=") {
            let v = &tag[r + 4..];
            let v = v.trim_start_matches(['"', '\'']);
            let v = v.split(['"', '\'']).next().unwrap_or("");
            if v.split_whitespace().any(|t| t == "icon") {
                return true;
            }
        }
        rest = &rest[i + 5..];
    }
    false
}

/// `html` with an icon link added after `<head>`, or `None` when it has no `<head>` to put it in.
fn with_icon_link(html: &str, icon: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let at = lower.find("<head")?;
    let close = at + lower[at..].find('>')? + 1;
    let mut out = String::with_capacity(html.len() + icon.len() + 40);
    out.push_str(&html[..close]);
    out.push_str(&format!("<link rel=\"icon\" type=\"image/png\" href=\"{icon}\">"));
    out.push_str(&html[close..]);
    Some(out)
}

/// A page of the build (`/`, `/dir/` or `*.html`) with the app's icon added when the page declares
/// none. `None` leaves the request to the normal file server (other files, pages that already
/// have an icon, missing files, anything odd).
async fn html_with_icon(dir: &std::path::Path, path_only: &str, icon: &str) -> Option<Response> {
    if path_only.contains('%') || path_only.contains('\\') {
        return None;
    }
    let rel = path_only.trim_start_matches('/');
    let rel = if rel.is_empty() || rel.ends_with('/') { format!("{rel}index.html") } else { rel.to_string() };
    if !rel.to_ascii_lowercase().ends_with(".html") {
        return None;
    }
    let rel = std::path::Path::new(&rel);
    if !rel.components().all(|c| matches!(c, std::path::Component::Normal(_))) {
        return None;
    }
    let file = dir.join(rel);
    let meta = tokio::fs::metadata(&file).await.ok()?;
    if !meta.is_file() || meta.len() > MAX_HTML_BYTES {
        return None;
    }
    let html = String::from_utf8(tokio::fs::read(&file).await.ok()?).ok()?;
    if declares_icon(&html) {
        return None;
    }
    let body = with_icon_link(&html, icon)?;
    Some(
        (
            [(header::CONTENT_TYPE, "text/html; charset=utf-8"), (header::CACHE_CONTROL, "no-cache")],
            body,
        )
            .into_response(),
    )
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
    #[test]
    fn adds_an_icon_only_when_the_page_has_none() {
        assert!(!declares_icon("<head><title>x</title><link rel=\"stylesheet\" href=\"a.css\"></head>"));
        assert!(declares_icon("<head><link rel=\"icon\" href=\"f.png\"></head>"));
        assert!(declares_icon("<HEAD><LINK REL='Shortcut Icon' HREF='f.ico'></HEAD>"));
        assert!(declares_icon("<link href=f.png rel=icon>"));
        assert!(!declares_icon("<link rel=\"apple-touch-icon\" href=\"a.png\">"));
        let out = with_icon_link("<!doctype html><html><head lang=\"en\"><title>x</title></head>", "/api/apps/a/icon").unwrap();
        assert!(out.contains("<head lang=\"en\"><link rel=\"icon\" type=\"image/png\" href=\"/api/apps/a/icon\"><title>"));
        assert_eq!(with_icon_link("<html><body></body></html>", "/i"), None);
    }
}
