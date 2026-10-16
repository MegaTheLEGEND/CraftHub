//! JSON API used by the web UI.

use crate::auth::SessionUser;
use crate::brand;
use crate::config::{self, AppDef, AuthCfg, AuthMode, SavedAuth, is_safe_name};
use crate::github::{self, Release};
use crate::manager;
use crate::state::{AppState, Installed, Job, fingerprint, now};
use artcraft_launcher_core::{
    AppStatus, CraftApp, Installed as CoreInstalled, InstallSource, Latest, catalog, details, status, version,
};
use axum::{
    Extension, Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::build_info;

fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(json!({ "error": msg.into() }))).into_response()
}

/// Mutating calls need an admin and a custom header (browsers will not add it cross-site).
fn guard_admin(user: &SessionUser, headers: &HeaderMap) -> Result<(), Response> {
    if !user.admin {
        return Err(err(StatusCode::FORBIDDEN, "administrator access required"));
    }
    match headers.get("x-requested-with").and_then(|v| v.to_str().ok()) {
        Some(v) if v == brand::REQUESTED_WITH => Ok(()),
        _ => Err(err(StatusCode::BAD_REQUEST, "missing X-Requested-With header")),
    }
}

#[derive(Serialize)]
pub struct LatestView {
    tag: String,
    version: String,
    prerelease: bool,
    published_at: Option<i64>,
    url: String,
}

#[derive(Serialize)]
pub struct AppView {
    id: String,
    name: String,
    tagline: String,
    repo: String,
    icon: String,
    /// classic | new | community
    group: &'static str,
    /// 1-based position in the suite (0 for community apps).
    number: usize,
    color: Option<String>,
    ink_light: Option<String>,
    ink_dark: Option<String>,
    inspired_by: Option<&'static str>,
    website: Option<String>,
    has_details: bool,
    /// Added from the UI; can be removed from the UI.
    custom: bool,
    /// checking | available | up_to_date | update_available | installed | unavailable | no_release | failed
    status: &'static str,
    /// An administrator rolled back to an older version on purpose.
    pinned: bool,
    active: Option<String>,
    installed: Vec<Installed>,
    auto_update: bool,
    include_prerelease: bool,
    latest: Option<LatestView>,
    /// Desktop downloads, one best choice per visitor platform (`windows-x64`, `macos`, ...).
    downloads: std::collections::BTreeMap<&'static str, github::Native>,
    native: Vec<github::Native>,
    job: Option<Job>,
    last_check: Option<u64>,
    last_error: Option<String>,
}

fn hex(c: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
}

async fn app_view(st: &AppState, id: &str) -> Option<AppView> {
    let def = st.app_def(id)?;
    let rec = st.store.record(id).await;
    let pre = manager::effective_prerelease(st, rec.include_prerelease);
    let releases = st.cached_releases(id).await;
    let latest = st.latest(id, pre).await;
    // Desktop downloads come from the newest release that has any, even if it lacks a web build.
    let native_rel: Option<Release> = releases
        .iter()
        .find(|r| (pre || !r.prerelease) && !r.native.is_empty())
        .cloned()
        .or_else(|| latest.clone().filter(|l| !l.native.is_empty()));
    let stems = github::stems_for(&def);
    let stem_refs: Vec<&str> = stems.iter().map(String::as_str).collect();
    let downloads = native_rel.as_ref().map(|r| github::downloads_for(r, &stem_refs)).unwrap_or_default();
    let native = native_rel.map(|r| r.native).unwrap_or_default();

    let installed = rec.active.as_deref().map(|v| CoreInstalled {
        version: version::parse(v),
        source: InstallSource::Launcher,
        location: String::new(),
        package: None,
    });
    let lat = match &latest {
        Some(l) => Latest::Found { version: l.semver.as_ref(), has_asset: true },
        None if !releases.is_empty() => Latest::Found { version: None, has_asset: false },
        None if rec.last_error.is_some() => Latest::Failed,
        None if rec.last_check.is_some() => Latest::NoRelease,
        None => Latest::Pending,
    };
    let mut s = status::status(installed.as_ref(), lat);
    let pinned = s == AppStatus::UpdateAvailable && latest.as_ref().map(|l| rec.installed.iter().any(|i| i.version == l.version)).unwrap_or(false);
    if pinned {
        s = AppStatus::UpToDate;
    }
    let status_str = match s {
        AppStatus::Checking => "checking",
        AppStatus::Available => "available",
        AppStatus::UpToDate => "up_to_date",
        AppStatus::UpdateAvailable => "update_available",
        AppStatus::Installed => "installed",
        AppStatus::Unavailable if rec.last_error.is_some() && releases.is_empty() => "failed",
        AppStatus::Unavailable => "unavailable",
        AppStatus::NoRelease => "no_release",
    };

    let suite = CraftApp::by_id(&def.id);
    let suite_index = catalog::SUITE.iter().position(|a| a.id == def.id);
    let group = match suite.map(|a| a.group) {
        Some(catalog::AppGroup::Classic) => "classic",
        Some(catalog::AppGroup::New) => "new",
        None => "community",
    };
    Some(AppView {
        id: def.id.clone(),
        name: def.name.clone(),
        tagline: def.description.clone(),
        repo: def.repo.clone(),
        // Served (and cached) by this server so browsers never contact GitHub for icons.
        icon: format!("/api/apps/{}/icon", def.id),
        group,
        number: suite_index.map(|i| i + 1).unwrap_or(0),
        color: suite.map(|a| hex(a.color)),
        ink_light: suite.map(|a| hex(a.ink_light)),
        ink_dark: suite.map(|a| hex(a.ink_dark)),
        inspired_by: if st.cfg.show_inspired_by { config::inspired_by(&def.id) } else { None },
        website: suite.map(|a| a.website()),
        has_details: details::AppDetails::by_id(&def.id).is_some(),
        custom: def.custom,
        status: status_str,
        pinned,
        active: rec.active.clone(),
        installed: rec.installed.clone(),
        auto_update: rec.auto_update,
        include_prerelease: pre,
        latest: latest.map(|l| LatestView { tag: l.tag, version: l.version, prerelease: l.prerelease, published_at: l.published_at, url: l.url }),
        downloads,
        native,
        job: st.job(id),
        last_check: rec.last_check,
        last_error: rec.last_error.clone(),
    })
}

pub async fn me(State(st): State<AppState>, Extension(user): Extension<SessionUser>) -> Json<serde_json::Value> {
    Json(json!({
        "name": user.name,
        "email": user.email,
        "admin": user.admin,
        "can_sign_out": st.auth().mode != AuthMode::None,
        "version": env!("CARGO_PKG_VERSION"),
        "update_interval_minutes": st.cfg.update_interval_minutes,
        "build_time": build_info::BUILD_TIME,
        "auth_mode": match st.auth().mode { AuthMode::Oidc => "oidc", AuthMode::Headers => "headers", AuthMode::None => "none" },
        "brand": { "name": brand::PRODUCT, "wordmark": brand::WORDMARK, "tag": brand::TAG, "requested_with": brand::REQUESTED_WITH, "repo": brand::REPO_URL },
        "show_inspired_by": st.cfg.show_inspired_by,
        "feed_enabled": st.cfg.feed_url.is_some(),
        "suite": { "website": catalog::WEBSITE_URL, "github": catalog::GITHUB_URL, "discord": catalog::DISCORD_URL, "social": catalog::SOCIAL },
    }))
}

fn notice_json(n: &artcraft_launcher_core::feed::Notice) -> serde_json::Value {
    json!({ "id": n.id, "title": n.title, "body": n.body, "link": n.link, "created_at": n.created_at })
}

pub async fn list_apps(State(st): State<AppState>) -> Json<serde_json::Value> {
    let mut out = vec![];
    for def in st.catalog_snapshot() {
        if let Some(v) = app_view(&st, &def.id).await {
            out.push(v);
        }
    }
    let (announcements, alerts) = match st.feed.lock().unwrap().as_ref() {
        Some((_, f)) => (f.announcements.iter().map(notice_json).collect::<Vec<_>>(), f.alerts.iter().map(notice_json).collect::<Vec<_>>()),
        None => (vec![], vec![]),
    };
    let limited = (*st.rate_limited_until.lock().unwrap()).filter(|t| *t > now());
    Json(json!({
        "apps": out,
        "meta": {
            "last_check": *st.last_full_check.lock().unwrap(),
            "sources": *st.last_sources.lock().unwrap(),
            "rate_limited_until": limited,
            "now": now(),
            "announcements": announcements,
            "alerts": alerts,
        }
    }))
}

#[derive(Deserialize)]
pub struct AddAppBody {
    /// `owner/repo` or any github.com URL of the repository.
    repo: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    description: String,
    /// Substring that identifies the web-build zip among release assets (default `-web-`).
    #[serde(default)]
    web_asset_contains: String,
}

/// Add a GitHub repository to the catalog. It is listed but never installed until an admin presses Install.
pub async fn add_app(
    State(st): State<AppState>,
    Extension(user): Extension<SessionUser>,
    headers: HeaderMap,
    Json(b): Json<AddAppBody>,
) -> Response {
    if let Err(r) = guard_admin(&user, &headers) {
        return r;
    }
    let repo = match config::parse_repo(&b.repo) {
        Ok(r) => r,
        Err(e) => return err(StatusCode::BAD_REQUEST, format!("{e:#}")),
    };
    if st.catalog_snapshot().iter().any(|a| a.repo.eq_ignore_ascii_case(&repo)) {
        return err(StatusCode::CONFLICT, "that repository is already in the catalog");
    }
    let marker = b.web_asset_contains.trim();
    let marker = if marker.is_empty() { "-web-".to_string() } else { marker.to_string() };
    if marker.len() > 100 || marker.chars().any(|c| c.is_control()) {
        return err(StatusCode::BAD_REQUEST, "invalid web asset marker");
    }
    let info = match st.repo_info(&repo).await {
        Ok(i) => i,
        Err(e) => return err(StatusCode::BAD_GATEWAY, format!("{e:#}")),
    };
    let repo = if info.full_name.is_empty() { repo } else { info.full_name.clone() };
    let clip = |s: &str, n: usize| -> String { s.trim().chars().filter(|c| !c.is_control()).take(n).collect() };
    let name = {
        let n = clip(&b.name, 60);
        if n.is_empty() { clip(&info.name, 60) } else { n }
    };
    let description = {
        let d = clip(&b.description, 300);
        if d.is_empty() { clip(&info.description, 300) } else { d }
    };
    let snapshot = st.catalog_snapshot();
    let id = config::derive_id(&repo, &snapshot);
    // Hosted apps run on this origin, so a new app starts with auto-update off and is only installed on request.
    if let Err(e) = st.store.update(&id, |r| r.auto_update = false).await {
        return err(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}"));
    }
    {
        let mut cat = st.catalog.write().unwrap();
        if cat.iter().any(|a| a.id == id || a.repo.eq_ignore_ascii_case(&repo)) {
            return err(StatusCode::CONFLICT, "that app was just added; reload the page");
        }
        let mut next = cat.clone();
        next.push(AppDef {
            id: id.clone(),
            name,
            repo,
            description,
            web_asset_contains: marker,
            icon: None,
            disabled: false,
            custom: true,
        });
        if let Err(e) = config::save_custom(&st.cfg.data_dir, &next) {
            return err(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}"));
        }
        *cat = next;
    }
    tracing::info!(app = %id, "app added from the UI");
    let st2 = st.clone();
    let id2 = id.clone();
    tokio::spawn(async move {
        match st2.releases(&id2, true).await {
            Ok(_) => {
                let _ = st2
                    .store
                    .update(&id2, |r| {
                        r.last_check = Some(now());
                        r.last_error = None;
                    })
                    .await;
            }
            Err(e) => {
                let m = format!("{e:#}");
                let _ = st2.store.update(&id2, |r| r.last_error = Some(m)).await;
            }
        }
    });
    match app_view(&st, &id).await {
        Some(v) => Json(v).into_response(),
        None => err(StatusCode::INTERNAL_SERVER_ERROR, "app disappeared"),
    }
}

/// Remove an app that was added from the UI, together with its installed versions.
pub async fn remove_app(
    State(st): State<AppState>,
    Extension(user): Extension<SessionUser>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if let Err(r) = guard_admin(&user, &headers) {
        return r;
    }
    let Some(def) = st.app_def(&id) else {
        return err(StatusCode::NOT_FOUND, "unknown app");
    };
    if !def.custom {
        return err(
            StatusCode::BAD_REQUEST,
            "built-in apps cannot be removed here; hide one with `disabled = true` in /data/apps.toml",
        );
    }
    if st.job_running(&id) {
        return err(StatusCode::CONFLICT, "an install is running for this app");
    }
    {
        let mut cat = st.catalog.write().unwrap();
        let next: Vec<AppDef> = cat.iter().filter(|a| a.id != def.id).cloned().collect();
        if let Err(e) = config::save_custom(&st.cfg.data_dir, &next) {
            return err(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}"));
        }
        *cat = next;
    }
    let _ = st.store.remove(&def.id).await;
    st.releases.lock().await.remove(&def.id);
    st.jobs.lock().unwrap().remove(&def.id);
    let _ = tokio::fs::remove_dir_all(st.apps_dir().join(&def.id)).await;
    let icons = st.cfg.data_dir.join("icons");
    let _ = tokio::fs::remove_file(icons.join(format!("{}.png", def.id))).await;
    let _ = tokio::fs::remove_file(icons.join(format!("{}.none", def.id))).await;
    tracing::info!(app = %def.id, "app removed from the UI");
    StatusCode::NO_CONTENT.into_response()
}

pub async fn check_all(
    State(st): State<AppState>,
    Extension(user): Extension<SessionUser>,
    headers: HeaderMap,
) -> Response {
    if let Err(r) = guard_admin(&user, &headers) {
        return r;
    }
    tokio::spawn(async move { manager::check_all(&st).await });
    StatusCode::ACCEPTED.into_response()
}

pub async fn refresh(
    State(st): State<AppState>,
    Extension(user): Extension<SessionUser>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if let Err(r) = guard_admin(&user, &headers) {
        return r;
    }
    if st.app_def(&id).is_none() {
        return err(StatusCode::NOT_FOUND, "unknown app");
    }
    // Refresh the list only; installing stays an explicit action here.
    match st.releases(&id, true).await {
        Ok(_) => {
            let _ = st.store.update(&id, |r| {
                r.last_check = Some(crate::state::now());
                r.last_error = None;
            }).await;
            Json(app_view(&st, &id).await).into_response()
        }
        Err(e) => {
            let msg = format!("{e:#}");
            let m = msg.clone();
            let _ = st.store.update(&id, |r| r.last_error = Some(m)).await;
            err(StatusCode::BAD_GATEWAY, msg)
        }
    }
}

#[derive(Serialize)]
struct ReleaseView {
    tag: String,
    version: String,
    name: String,
    prerelease: bool,
    published_at: Option<i64>,
    url: String,
    notes: String,
    web: Option<serde_json::Value>,
    native: usize,
}

pub async fn releases(State(st): State<AppState>, Path(id): Path<String>) -> Response {
    if st.app_def(&id).is_none() {
        return err(StatusCode::NOT_FOUND, "unknown app");
    }
    match st.releases(&id, false).await {
        Ok(list) => Json(
            list.into_iter()
                .map(|r| ReleaseView {
                    web: r.web.as_ref().map(|w| json!({ "name": w.name, "size": w.size, "verified": w.sha256.is_some() || r.sums_url.is_some() })),
                    native: r.native.len(),
                    tag: r.tag,
                    version: r.version,
                    name: r.name,
                    prerelease: r.prerelease,
                    published_at: r.published_at,
                    url: r.url,
                    notes: r.notes,
                })
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(e) => err(StatusCode::BAD_GATEWAY, format!("{e:#}")),
    }
}

#[derive(Deserialize)]
pub struct InstallBody {
    tag: String,
    #[serde(default = "yes")]
    activate: bool,
}

fn yes() -> bool {
    true
}

pub async fn install(
    State(st): State<AppState>,
    Extension(user): Extension<SessionUser>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<InstallBody>,
) -> Response {
    if let Err(r) = guard_admin(&user, &headers) {
        return r;
    }
    if st.app_def(&id).is_none() {
        return err(StatusCode::NOT_FOUND, "unknown app");
    }
    if st.job_running(&id) {
        return err(StatusCode::CONFLICT, "an install is already running for this app");
    }
    // Mark the job as started synchronously so the UI sees it on its next poll.
    st.set_job(&id, |j| {
        j.state = "downloading".into();
        j.tag = body.tag.clone();
        j.message = "Starting".into();
        j.done = 0;
        j.total = 0;
    });
    let st2 = st.clone();
    tokio::spawn(async move {
        let _ = manager::install(st2, id, body.tag, body.activate).await;
    });
    StatusCode::ACCEPTED.into_response()
}

#[derive(Deserialize)]
pub struct VersionBody {
    version: String,
}

pub async fn activate(
    State(st): State<AppState>,
    Extension(user): Extension<SessionUser>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<VersionBody>,
) -> Response {
    if let Err(r) = guard_admin(&user, &headers) {
        return r;
    }
    if st.app_def(&id).is_none() || !is_safe_name(&body.version) {
        return err(StatusCode::NOT_FOUND, "unknown app or version");
    }
    match manager::activate(&st, &id, &body.version).await {
        Ok(()) => Json(app_view(&st, &id).await).into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, format!("{e:#}")),
    }
}

pub async fn remove_version(
    State(st): State<AppState>,
    Extension(user): Extension<SessionUser>,
    headers: HeaderMap,
    Path((id, version)): Path<(String, String)>,
) -> Response {
    if let Err(r) = guard_admin(&user, &headers) {
        return r;
    }
    if st.app_def(&id).is_none() || !is_safe_name(&version) {
        return err(StatusCode::NOT_FOUND, "unknown app or version");
    }
    match manager::remove(&st, &id, &version).await {
        Ok(()) => Json(app_view(&st, &id).await).into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, format!("{e:#}")),
    }
}

#[derive(Deserialize)]
pub struct SettingsBody {
    auto_update: Option<bool>,
    include_prerelease: Option<bool>,
}

pub async fn settings(
    State(st): State<AppState>,
    Extension(user): Extension<SessionUser>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<SettingsBody>,
) -> Response {
    if let Err(r) = guard_admin(&user, &headers) {
        return r;
    }
    if st.app_def(&id).is_none() {
        return err(StatusCode::NOT_FOUND, "unknown app");
    }
    let res = st
        .store
        .update(&id, |r| {
            if let Some(a) = body.auto_update {
                r.auto_update = a;
            }
            if let Some(p) = body.include_prerelease {
                r.include_prerelease = Some(p);
            }
        })
        .await;
    match res {
        Ok(()) => Json(app_view(&st, &id).await).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")),
    }
}

pub async fn get_settings(State(st): State<AppState>, Extension(user): Extension<SessionUser>) -> Response {
    if !user.admin {
        return err(StatusCode::FORBIDDEN, "administrator access required");
    }
    Json(json!({ "github_token_set": st.github_token().is_some(), "github_token_source": st.token_source() })).into_response()
}

#[derive(Deserialize)]
pub struct SettingsPatch {
    /// A token to store, or empty/null to remove the UI-set token.
    github_token: Option<String>,
}

pub async fn set_settings(
    State(st): State<AppState>,
    Extension(user): Extension<SessionUser>,
    headers: HeaderMap,
    Json(body): Json<SettingsPatch>,
) -> Response {
    if let Err(r) = guard_admin(&user, &headers) {
        return r;
    }
    let token = body.github_token.map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
    if let Some(t) = &token {
        if t.len() > 255 || t.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return err(StatusCode::BAD_REQUEST, "that does not look like a GitHub token");
        }
        let url = format!("{}/rate_limit", st.cfg.github_api.trim_end_matches('/'));
        match st.http.get(&url).bearer_auth(t).header("Accept", "application/vnd.github+json").send().await {
            Ok(r) if r.status().as_u16() == 401 => return err(StatusCode::BAD_REQUEST, "GitHub rejected this token"),
            Ok(_) => {}
            Err(e) => return err(StatusCode::BAD_GATEWAY, format!("could not reach GitHub to check the token: {e}")),
        }
    }
    if let Err(e) = st.set_ui_token(token) {
        return err(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}"));
    }
    let st2 = st.clone();
    tokio::spawn(async move { manager::check_all(&st2).await });
    Json(json!({ "github_token_set": st.github_token().is_some(), "github_token_source": st.token_source() })).into_response()
}

// ---------------------------------------------------------------------------
// Authentication settings
// ---------------------------------------------------------------------------

fn mode_str(m: &AuthMode) -> &'static str {
    match m {
        AuthMode::Oidc => "oidc",
        AuthMode::Headers => "headers",
        AuthMode::None => "none",
    }
}

fn auth_view(st: &AppState) -> serde_json::Value {
    let a = st.auth();
    let (saved, draft, tested) = {
        let g = st.settings.lock().unwrap();
        (g.auth.clone(), g.draft.clone(), g.tested.clone())
    };
    // The form edits the draft if there is one, otherwise what is active now.
    let form = draft.clone().unwrap_or_else(|| SavedAuth {
        mode: "oidc".into(),
        public_url: a.public_url.clone(),
        oidc_issuer: a.oidc_issuer.clone(),
        oidc_client_id: a.oidc_client_id.clone(),
        oidc_client_secret: a.oidc_client_secret.clone(),
        oidc_scopes: a.oidc_scopes.clone(),
        groups_claim: a.groups_claim.clone(),
        allowed_groups: a.allowed_groups.clone(),
        admin_groups: a.admin_groups.clone(),
    });
    let tested_view = tested.as_ref().map(|t| {
        let fresh = now().saturating_sub(t.at) < 1800;
        let matches = draft.as_ref().map(|d| fingerprint(d) == t.fingerprint).unwrap_or(false);
        json!({ "name": t.name, "groups": t.groups, "admin": t.admin, "at": t.at, "ok": t.admin && fresh && matches, "stale": !fresh || !matches })
    });
    json!({
        "mode": mode_str(&a.mode),
        "source": if st.cfg.auth_recovery { "recovery" } else if saved.is_some() { "ui" } else { "env" },
        "redirect_uri": format!("{}/auth/callback", if form.public_url.is_empty() { &a.public_url } else { &form.public_url }),
        "has_draft": draft.is_some(),
        "active": { "admin_groups": a.admin_groups, "allowed_groups": a.allowed_groups },
        "form": {
            "public_url": form.public_url,
            "oidc_issuer": form.oidc_issuer,
            "oidc_client_id": form.oidc_client_id,
            "secret_set": !form.oidc_client_secret.is_empty(),
            "oidc_scopes": form.oidc_scopes,
            "groups_claim": form.groups_claim,
            "allowed_groups": form.allowed_groups,
            "admin_groups": form.admin_groups,
        },
        "tested": tested_view,
    })
}

pub async fn get_auth(State(st): State<AppState>, Extension(user): Extension<SessionUser>) -> Response {
    if !user.admin {
        return err(StatusCode::FORBIDDEN, "administrator access required");
    }
    Json(auth_view(&st)).into_response()
}

#[derive(Deserialize)]
pub struct DraftBody {
    public_url: String,
    oidc_issuer: String,
    oidc_client_id: String,
    /// Empty keeps the stored secret.
    #[serde(default)]
    oidc_client_secret: String,
    #[serde(default)]
    oidc_scopes: String,
    #[serde(default)]
    groups_claim: String,
    #[serde(default)]
    allowed_groups: Vec<String>,
    #[serde(default)]
    admin_groups: Vec<String>,
}

fn clean_list(v: Vec<String>) -> Vec<String> {
    v.into_iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()
}

pub async fn save_auth_draft(
    State(st): State<AppState>,
    Extension(user): Extension<SessionUser>,
    headers: HeaderMap,
    Json(b): Json<DraftBody>,
) -> Response {
    if let Err(r) = guard_admin(&user, &headers) {
        return r;
    }
    let public_url = b.public_url.trim().trim_end_matches('/').to_string();
    let issuer = b.oidc_issuer.trim().to_string();
    if !(public_url.starts_with("http://") || public_url.starts_with("https://")) || !(issuer.starts_with("http://") || issuer.starts_with("https://")) {
        return err(StatusCode::BAD_REQUEST, "public URL and issuer must start with http:// or https://");
    }
    if b.oidc_client_id.trim().is_empty() {
        return err(StatusCode::BAD_REQUEST, "client ID is required");
    }
    let prev_secret = {
        let g = st.settings.lock().unwrap();
        g.draft.as_ref().map(|d| d.oidc_client_secret.clone()).filter(|s| !s.is_empty())
            .or_else(|| g.auth.as_ref().map(|d| d.oidc_client_secret.clone()).filter(|s| !s.is_empty()))
    }
    .or_else(|| Some(st.auth().oidc_client_secret.clone()).filter(|s| !s.is_empty()));
    let secret = if b.oidc_client_secret.trim().is_empty() { prev_secret.unwrap_or_default() } else { b.oidc_client_secret.trim().to_string() };
    if secret.is_empty() {
        return err(StatusCode::BAD_REQUEST, "client secret is required");
    }
    let draft = SavedAuth {
        mode: "oidc".into(),
        public_url,
        oidc_issuer: issuer,
        oidc_client_id: b.oidc_client_id.trim().to_string(),
        oidc_client_secret: secret,
        oidc_scopes: if b.oidc_scopes.trim().is_empty() { "openid profile email".into() } else { b.oidc_scopes.trim().to_string() },
        groups_claim: if b.groups_claim.trim().is_empty() { "groups".into() } else { b.groups_claim.trim().to_string() },
        allowed_groups: clean_list(b.allowed_groups),
        admin_groups: clean_list(b.admin_groups),
    };
    // Fail early on a wrong issuer instead of at the test sign-in.
    let url = format!("{}/.well-known/openid-configuration", draft.oidc_issuer.trim_end_matches('/'));
    match st.http.get(&url).send().await.and_then(|r| r.error_for_status()) {
        Ok(_) => {}
        Err(e) => return err(StatusCode::BAD_REQUEST, format!("could not read {url}: {e}")),
    }
    if let Err(e) = st.update_settings(|s| {
        s.draft = Some(draft);
        s.tested = None;
    }) {
        return err(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}"));
    }
    Json(auth_view(&st)).into_response()
}

pub async fn enable_auth(State(st): State<AppState>, Extension(user): Extension<SessionUser>, headers: HeaderMap) -> Response {
    if let Err(r) = guard_admin(&user, &headers) {
        return r;
    }
    let (draft, tested) = {
        let g = st.settings.lock().unwrap();
        (g.draft.clone(), g.tested.clone())
    };
    let (Some(draft), Some(tested)) = (draft, tested) else {
        return err(StatusCode::BAD_REQUEST, "save the settings and run a successful test sign-in first");
    };
    if tested.fingerprint != fingerprint(&draft) || now().saturating_sub(tested.at) > 1800 || !tested.admin {
        return err(StatusCode::BAD_REQUEST, "the last test sign-in is missing, outdated, or the account was not an administrator; test again");
    }
    let cfg = AuthCfg::from_saved(&st.cfg, &draft);
    if let Err(e) = st.update_settings(|s| {
        s.auth = Some(draft);
        s.draft = None;
        s.tested = None;
    }) {
        return err(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}"));
    }
    st.set_auth(cfg);
    tracing::info!("authentication enabled from the UI (OIDC)");
    Json(auth_view(&st)).into_response()
}

pub async fn disable_auth(State(st): State<AppState>, Extension(user): Extension<SessionUser>, headers: HeaderMap) -> Response {
    if let Err(r) = guard_admin(&user, &headers) {
        return r;
    }
    let off = SavedAuth { mode: "none".into(), ..Default::default() };
    // Keep the OIDC details as a draft so it can be re-enabled after a new test.
    let prev = st.settings.lock().unwrap().auth.clone().filter(|a| a.mode == "oidc");
    if let Err(e) = st.update_settings(|s| {
        s.auth = Some(off.clone());
        if let Some(p) = prev {
            s.draft = Some(p);
        }
        s.tested = None;
    }) {
        return err(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}"));
    }
    st.set_auth(AuthCfg::from_saved(&st.cfg, &off));
    tracing::warn!("authentication disabled from the UI");
    Json(auth_view(&st)).into_response()
}

/// Install every app that has an update. Runs one install after the other.
pub async fn update_all(State(st): State<AppState>, Extension(user): Extension<SessionUser>, headers: HeaderMap) -> Response {
    if let Err(r) = guard_admin(&user, &headers) {
        return r;
    }
    let mut targets = vec![];
    for def in st.catalog_snapshot() {
        if st.job_running(&def.id) {
            continue;
        }
        if let Some(v) = app_view(&st, &def.id).await {
            if v.status == "update_available" {
                if let Some(l) = v.latest {
                    targets.push((def.id.clone(), l.tag));
                }
            }
        }
    }
    let n = targets.len();
    let st2 = st.clone();
    tokio::spawn(async move { manager::update_all(st2, targets).await });
    (StatusCode::ACCEPTED, Json(json!({ "queued": n }))).into_response()
}

/// Stop a running (or queued) install.
pub async fn cancel(
    State(st): State<AppState>,
    Extension(user): Extension<SessionUser>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if let Err(r) = guard_admin(&user, &headers) {
        return r;
    }
    if st.app_def(&id).is_none() {
        return err(StatusCode::NOT_FOUND, "unknown app");
    }
    if !st.job_running(&id) {
        return err(StatusCode::CONFLICT, "nothing is installing");
    }
    st.cancels.lock().unwrap().insert(id.clone());
    // A queued app has no download loop to notice the flag, so mark it here.
    if st.job(&id).map(|j| j.state == "queued").unwrap_or(false) {
        st.set_job(&id, |j| {
            j.state = "cancelled".into();
            j.message = "Install cancelled".into();
        });
    }
    StatusCode::ACCEPTED.into_response()
}

pub async fn uninstall(
    State(st): State<AppState>,
    Extension(user): Extension<SessionUser>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if let Err(r) = guard_admin(&user, &headers) {
        return r;
    }
    if st.app_def(&id).is_none() {
        return err(StatusCode::NOT_FOUND, "unknown app");
    }
    if st.job_running(&id) {
        return err(StatusCode::CONFLICT, "an install is running for this app");
    }
    match manager::uninstall(&st, &id).await {
        Ok(()) => Json(app_view(&st, &id).await).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")),
    }
}

/// The hand-written product page of a suite app (compiled into the launcher core).
pub async fn app_details(Path(id): Path<String>) -> Response {
    let Some(d) = details::AppDetails::by_id(&id) else {
        return err(StatusCode::NOT_FOUND, "no details for this app");
    };
    Json(json!({
        "headline": d.headline,
        "paragraphs": d.paragraphs().collect::<Vec<_>>(),
        "features": d.features.iter().map(|f| json!({ "title": f.title, "text": f.text })).collect::<Vec<_>>(),
        "shots": d.shots.iter().map(|s| json!({ "key": s.key, "caption": s.caption, "url": format!("/api/apps/{}/shots/{}", d.id, s.key) })).collect::<Vec<_>>(),
        "platforms": d.platforms,
    }))
    .into_response()
}

/// A details-page picture, fetched once by the server and cached on disk.
pub async fn screenshot(State(st): State<AppState>, Path((id, key)): Path<(String, String)>) -> Response {
    let Some(d) = details::AppDetails::by_id(&id) else { return StatusCode::NOT_FOUND.into_response() };
    let Some(url) = d.screenshot_url(&key) else { return StatusCode::NOT_FOUND.into_response() };
    if !is_safe_name(&key) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let dir = st.cfg.data_dir.join("shots").join(details::SCREENSHOTS_REV).join(&id);
    let file = dir.join(format!("{key}.jpg"));
    let headers = [(axum::http::header::CONTENT_TYPE, "image/jpeg"), (axum::http::header::CACHE_CONTROL, "private, max-age=604800")];
    if let Ok(bytes) = tokio::fs::read(&file).await {
        return (headers, bytes).into_response();
    }
    let got = match st.http.get(&url).send().await.and_then(|r| r.error_for_status()) {
        Ok(r) => r.bytes().await.ok().filter(|b| b.len() <= details::MAX_SCREENSHOT_BYTES && details::looks_like_jpeg(b)),
        Err(_) => None,
    };
    match got {
        Some(bytes) => {
            let _ = tokio::fs::create_dir_all(&dir).await;
            let _ = tokio::fs::write(&file, &bytes).await;
            (headers, bytes.to_vec()).into_response()
        }
        None => (StatusCode::NOT_FOUND, [(axum::http::header::CACHE_CONTROL, "private, max-age=300")]).into_response(),
    }
}

/// Icon for an app. Built-in apps ship theirs inside the binary when `ui/icons/<id>.png` exists; others are fetched once by the server
/// from the app's repository and cached on disk (failures are remembered for a day).
pub async fn icon(State(st): State<AppState>, Path(id): Path<String>) -> Response {
    let Some(def) = st.app_def(&id) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let headers = [(axum::http::header::CONTENT_TYPE, "image/png"), (axum::http::header::CACHE_CONTROL, "private, max-age=86400")];
    // Built-in apps (the suite, SolidCraft) use the icon bundled in the binary when there is one.
    if !def.custom {
        if let Some(f) = crate::assets::file(&format!("icons/{}.png", def.id)) {
            return (headers, f.contents().to_vec()).into_response();
        }
    }
    let dir = st.cfg.data_dir.join("icons");
    let file = dir.join(format!("{id}.png"));
    let miss = dir.join(format!("{id}.none"));
    if let Ok(bytes) = tokio::fs::read(&file).await {
        return (headers, bytes).into_response();
    }
    if let Ok(meta) = tokio::fs::metadata(&miss).await {
        let fresh = meta.modified().ok().and_then(|m| m.elapsed().ok()).map(|e| e.as_secs() < 86400).unwrap_or(false);
        if fresh {
            return (StatusCode::NO_CONTENT, [(axum::http::header::CACHE_CONTROL, "private, max-age=3600")]).into_response();
        }
    }
    let url = def.icon.clone().unwrap_or_else(|| {
        format!("https://raw.githubusercontent.com/{}/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.{}.png", def.repo, def.id)
    });
    let _ = tokio::fs::create_dir_all(&dir).await;
    let got = match st.http.get(&url).send().await.and_then(|r| r.error_for_status()) {
        Ok(r) => r.bytes().await.ok().filter(|b| b.len() < 512 * 1024 && b.starts_with(b"\x89PNG")),
        Err(_) => None,
    };
    match got {
        Some(bytes) => {
            let _ = tokio::fs::write(&file, &bytes).await;
            (headers, bytes.to_vec()).into_response()
        }
        None => {
            let _ = tokio::fs::write(&miss, b"").await;
            (StatusCode::NO_CONTENT, [(axum::http::header::CACHE_CONTROL, "private, max-age=3600")]).into_response()
        }
    }
}
