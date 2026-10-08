//! JSON API used by the launcher UI.

use crate::auth::SessionUser;
use crate::config::{is_safe_name, AuthCfg, AuthMode, SavedAuth};
use crate::github::{pick_latest, NativeAsset, Release};
use crate::manager;
use crate::state::{fingerprint, now, AppState, Installed, Job};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Extension, Json,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(json!({ "error": msg.into() }))).into_response()
}

/// Mutating calls need an admin and a custom header (browsers will not add it cross-site).
fn guard_admin(user: &SessionUser, headers: &HeaderMap) -> Result<(), Response> {
    if !user.admin {
        return Err(err(StatusCode::FORBIDDEN, "administrator access required"));
    }
    match headers.get("x-requested-with").and_then(|v| v.to_str().ok()) {
        Some("craft-hub") => Ok(()),
        _ => Err(err(StatusCode::BAD_REQUEST, "missing X-Requested-With header")),
    }
}

#[derive(Serialize)]
pub struct LatestView {
    tag: String,
    version: String,
    prerelease: bool,
    published_at: Option<String>,
    url: String,
}

#[derive(Serialize)]
pub struct AppView {
    id: String,
    name: String,
    description: String,
    repo: String,
    icon: String,
    active: Option<String>,
    installed: Vec<Installed>,
    auto_update: bool,
    include_prerelease: bool,
    latest: Option<LatestView>,
    update_available: bool,
    native: Vec<NativeAsset>,
    job: Option<Job>,
    last_check: Option<u64>,
    last_error: Option<String>,
    checked: bool,
}

async fn app_view(st: &AppState, id: &str) -> Option<AppView> {
    let def = st.app_def(id)?;
    let rec = st.store.record(id).await;
    let releases = st.cached_releases(id).await;
    let pre = manager::effective_prerelease(st, rec.include_prerelease);
    let latest = pick_latest(&releases, pre);
    // Desktop downloads come from the newest release that has any, even if it lacks a web build.
    let native = releases
        .iter()
        .find(|r| (pre || !r.prerelease) && !r.native.is_empty())
        .map(|r| r.native.clone())
        .unwrap_or_default();
    let update_available = match (latest, &rec.active) {
        (Some(l), Some(a)) => &l.version != a && !rec.installed.iter().any(|i| i.version == l.version),
        (Some(_), None) => true,
        _ => false,
    };
    Some(AppView {
        id: def.id.clone(),
        name: def.name.clone(),
        description: def.description.clone(),
        repo: def.repo.clone(),
        icon: def.icon.clone().unwrap_or_else(|| {
            format!(
                "https://raw.githubusercontent.com/{}/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.{}.png",
                def.repo, def.id
            )
        }),
        active: rec.active.clone(),
        installed: rec.installed.clone(),
        auto_update: rec.auto_update,
        include_prerelease: pre,
        latest: latest.map(|l| LatestView {
            tag: l.tag.clone(),
            version: l.version.clone(),
            prerelease: l.prerelease,
            published_at: l.published_at.clone(),
            url: l.url.clone(),
        }),
        update_available,
        native,
        job: st.job(id),
        last_check: rec.last_check,
        last_error: rec.last_error.clone(),
        checked: rec.last_check.is_some(),
    })
}

pub async fn me(State(st): State<AppState>, Extension(user): Extension<SessionUser>) -> Json<serde_json::Value> {
    Json(json!({
        "name": user.name,
        "email": user.email,
        "admin": user.admin,
        "can_sign_out": st.auth().mode != AuthMode::None,
        "auth_mode": match st.auth().mode { AuthMode::Oidc => "oidc", AuthMode::Headers => "headers", AuthMode::None => "none" },
    }))
}

pub async fn list_apps(State(st): State<AppState>) -> Json<Vec<AppView>> {
    let mut out = vec![];
    for def in &st.catalog {
        if let Some(v) = app_view(&st, &def.id).await {
            out.push(v);
        }
    }
    Json(out)
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

pub async fn releases(State(st): State<AppState>, Path(id): Path<String>) -> Response {
    if st.app_def(&id).is_none() {
        return err(StatusCode::NOT_FOUND, "unknown app");
    }
    match st.releases(&id, false).await {
        Ok(list) => Json::<Vec<Release>>(list).into_response(),
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
