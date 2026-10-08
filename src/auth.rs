//! Authentication: OIDC authorization-code flow (PKCE) for Authentik and friends,
//! optional trusted-header mode for a proxy outpost, and encrypted session cookies.

use crate::config::{AuthCfg, AuthMode};
use crate::state::{fingerprint, now, AppState, OidcMeta, TestResult};
use anyhow::{anyhow, bail, Context, Result};
use axum::{
    extract::{Query, Request, State},
    http::{header, HeaderMap, StatusCode},
    middleware::Next,
    response::{Html, IntoResponse, Redirect, Response},
};
use axum_extra::extract::cookie::{Cookie, PrivateCookieJar, SameSite};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use jsonwebtoken::{jwk::JwkSet, Algorithm, DecodingKey, Validation};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{collections::HashMap, sync::Arc, time::{Duration, Instant}};

const SESSION_COOKIE: &str = "craft_session";
const LOGIN_COOKIE: &str = "craft_login";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionUser {
    pub sub: String,
    pub name: String,
    pub email: String,
    pub admin: bool,
    pub exp: u64,
}

#[derive(Serialize, Deserialize)]
struct LoginState {
    state: String,
    nonce: String,
    verifier: String,
    next: String,
    #[serde(default)]
    test: bool,
}

pub struct AppError(pub anyhow::Error);

impl<E: Into<anyhow::Error>> From<E> for AppError {
    fn from(e: E) -> Self {
        AppError(e.into())
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        tracing::warn!("request failed: {:#}", self.0);
        (StatusCode::BAD_GATEWAY, page("Sign-in problem", &format!("{:#}", self.0), true)).into_response()
    }
}

fn rand_token(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    rand::thread_rng().fill_bytes(&mut buf);
    URL_SAFE_NO_PAD.encode(buf)
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn page(title: &str, body: &str, retry: bool) -> Html<String> {
    let link = if retry { r#"<p><a href="/auth/login">Try again</a></p>"# } else { "" };
    Html(format!(
        r#"<!doctype html><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>{t}</title><style>body{{font:16px system-ui;background:#0f1115;color:#e6e8ee;display:grid;place-items:center;min-height:100vh;margin:0}}
main{{max-width:30rem;padding:2rem}}a{{color:#5aa2ff}}code{{color:#9aa3b2}}</style>
<main><h1>{t}</h1><p>{b}</p>{link}</main>"#,
        t = esc(title),
        b = esc(body)
    ))
}

/// Only allow same-site relative redirects.
fn safe_next(next: Option<&str>) -> String {
    match next {
        Some(n) if n.starts_with('/') && !n.starts_with("//") && !n.starts_with("/\\") && !n.starts_with("/auth/") => n.to_string(),
        _ => "/".into(),
    }
}

// ---------------------------------------------------------------------------
// Roles
// ---------------------------------------------------------------------------

/// Returns `Some(is_admin)` when the user may use the app at all.
pub fn evaluate(cfg: &AuthCfg, groups: &[String]) -> Option<bool> {
    let in_admin_group = !cfg.admin_groups.is_empty() && groups.iter().any(|g| cfg.admin_groups.contains(g));
    let admin = if cfg.admin_groups.is_empty() { true } else { in_admin_group };
    let allowed = cfg.allowed_groups.is_empty()
        || in_admin_group
        || groups.iter().any(|g| cfg.allowed_groups.contains(g));
    allowed.then_some(admin)
}

fn parse_groups(v: Option<&Value>) -> Vec<String> {
    match v {
        Some(Value::Array(a)) => a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect(),
        Some(Value::String(s)) => s
            .split(['|', ','])
            .map(|g| g.trim().to_string())
            .filter(|g| !g.is_empty())
            .collect(),
        _ => vec![],
    }
}

// ---------------------------------------------------------------------------
// Middleware
// ---------------------------------------------------------------------------

fn user_from_cookie(st: &AppState, headers: &HeaderMap) -> Option<SessionUser> {
    let jar = PrivateCookieJar::from_headers(headers, st.key.clone());
    let raw = jar.get(SESSION_COOKIE)?;
    let user: SessionUser = serde_json::from_str(raw.value()).ok()?;
    (user.exp > now()).then_some(user)
}

fn user_from_headers(a: &AuthCfg, headers: &HeaderMap) -> Option<SessionUser> {
    let get = |name: &str| headers.get(name).and_then(|v| v.to_str().ok()).map(str::to_string);
    let name = get(&a.header_user)?;
    let groups = parse_groups(get(&a.header_groups).map(Value::String).as_ref());
    let admin = evaluate(a, &groups)?;
    Some(SessionUser {
        sub: name.clone(),
        name,
        email: get(&a.header_email).unwrap_or_default(),
        admin,
        exp: u64::MAX,
    })
}

pub async fn require_auth(State(st): State<AppState>, mut req: Request, next: Next) -> Response {
    let a = st.auth();
    let user = match a.mode {
        AuthMode::None => Some(SessionUser {
            sub: "local".into(),
            name: "Local user".into(),
            email: String::new(),
            admin: true,
            exp: u64::MAX,
        }),
        AuthMode::Headers => user_from_headers(&a, req.headers()),
        AuthMode::Oidc => user_from_cookie(&st, req.headers()),
    };
    match user {
        Some(u) => {
            req.extensions_mut().insert(u);
            next.run(req).await
        }
        None if req.uri().path().starts_with("/api/") => {
            (StatusCode::UNAUTHORIZED, [(header::CONTENT_TYPE, "application/json")], r#"{"error":"unauthenticated"}"#)
                .into_response()
        }
        None if a.mode == AuthMode::Headers => (
            StatusCode::UNAUTHORIZED,
            page("Not signed in", "The reverse proxy did not send identity headers, or your account is not in an allowed group.", false),
        )
            .into_response(),
        None => {
            let pq = req.uri().path_and_query().map(|p| p.as_str()).unwrap_or("/");
            let target = format!("/auth/login?next={}", urlencode(pq));
            Redirect::to(&target).into_response()
        }
    }
}

fn urlencode(s: &str) -> String {
    reqwest::Url::parse_with_params("http://x/", [("q", s)])
        .map(|u| u.query().unwrap_or("").trim_start_matches("q=").to_string())
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// OIDC
// ---------------------------------------------------------------------------

async fn oidc_meta(st: &AppState, a: &AuthCfg) -> Result<Arc<OidcMeta>> {
    let mut slot = st.oidc.lock().await;
    let url = format!("{}/.well-known/openid-configuration", a.oidc_issuer.trim_end_matches('/'));
    if let Some((at, for_url, meta)) = slot.as_ref() {
        if at.elapsed() < Duration::from_secs(3600) && *for_url == url {
            return Ok(meta.clone());
        }
    }
    let doc: Value = st
        .http
        .get(&url)
        .send()
        .await
        .with_context(|| format!("could not reach the identity provider at {url}"))?
        .error_for_status()
        .with_context(|| format!("discovery failed at {url} (check OIDC_ISSUER)"))?
        .json()
        .await?;
    let s = |k: &str| doc.get(k).and_then(Value::as_str).map(str::to_string);
    let meta = Arc::new(OidcMeta {
        issuer: s("issuer").ok_or_else(|| anyhow!("discovery document has no issuer"))?,
        authorization_endpoint: s("authorization_endpoint").ok_or_else(|| anyhow!("no authorization_endpoint"))?,
        token_endpoint: s("token_endpoint").ok_or_else(|| anyhow!("no token_endpoint"))?,
        jwks_uri: s("jwks_uri").ok_or_else(|| anyhow!("no jwks_uri"))?,
        userinfo_endpoint: s("userinfo_endpoint"),
        end_session_endpoint: s("end_session_endpoint"),
    });
    *slot = Some((Instant::now(), url, meta.clone()));
    Ok(meta)
}

#[derive(Deserialize)]
pub struct LoginQuery {
    next: Option<String>,
}

fn short_cookie(name: &'static str, value: String, path: &'static str, secure: bool, max_age: i64) -> Cookie<'static> {
    Cookie::build((name, value))
        .path(path)
        .http_only(true)
        .secure(secure)
        .same_site(SameSite::Lax)
        .max_age(time::Duration::seconds(max_age))
        .build()
}

pub async fn login(
    State(st): State<AppState>,
    Query(q): Query<LoginQuery>,
    jar: PrivateCookieJar,
) -> Result<Response, AppError> {
    let a = st.auth();
    if a.mode != AuthMode::Oidc {
        return Ok(Redirect::to("/").into_response());
    }
    start_login(&st, &a, safe_next(q.next.as_deref()), false, jar).await
}

/// Admin-only: sign in once with the *draft* settings so a broken configuration can never lock everyone out.
pub async fn login_test(
    State(st): State<AppState>,
    axum::Extension(user): axum::Extension<SessionUser>,
    jar: PrivateCookieJar,
) -> Result<Response, AppError> {
    if !user.admin {
        return Ok((StatusCode::FORBIDDEN, page("Admins only", "Only administrators can test sign-in settings.", false)).into_response());
    }
    let a = draft_cfg(&st)?;
    start_login(&st, &a, "/".into(), true, jar).await
}

fn draft_cfg(st: &AppState) -> Result<AuthCfg> {
    let draft = st.settings.lock().unwrap().draft.clone().ok_or_else(|| anyhow!("no draft authentication settings saved yet"))?;
    let a = AuthCfg::from_saved(&st.cfg, &draft);
    if a.mode != AuthMode::Oidc || a.oidc_issuer.is_empty() || a.oidc_client_id.is_empty() || a.oidc_client_secret.is_empty() {
        bail!("the draft is incomplete: issuer, client ID and client secret are required");
    }
    Ok(a)
}

async fn start_login(st: &AppState, a: &AuthCfg, next: String, test: bool, jar: PrivateCookieJar) -> Result<Response, AppError> {
    let meta = oidc_meta(st, a).await?;
    let verifier = rand_token(48);
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let ls = LoginState {
        state: rand_token(24),
        nonce: rand_token(24),
        verifier,
        next,
        test,
    };
    let redirect_uri = format!("{}/auth/callback", a.public_url);
    let mut url = reqwest::Url::parse(&meta.authorization_endpoint)?;
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", &a.oidc_client_id)
        .append_pair("redirect_uri", &redirect_uri)
        .append_pair("scope", &a.oidc_scopes)
        .append_pair("state", &ls.state)
        .append_pair("nonce", &ls.nonce)
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "S256");
    let cookie = short_cookie(LOGIN_COOKIE, serde_json::to_string(&ls)?, "/auth", a.cookie_secure, 600);
    Ok((jar.add(cookie), Redirect::to(url.as_str())).into_response())
}

#[derive(Deserialize)]
pub struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

#[derive(Deserialize)]
struct TokenResponse {
    id_token: String,
    #[serde(default)]
    access_token: Option<String>,
}

#[derive(Deserialize)]
struct IdClaims {
    sub: String,
    #[serde(default)]
    nonce: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    preferred_username: Option<String>,
    #[serde(default)]
    email: Option<String>,
    #[serde(flatten)]
    extra: HashMap<String, Value>,
}

async fn verify_id_token(st: &AppState, a: &AuthCfg, meta: &OidcMeta, token: &str) -> Result<IdClaims> {
    let header = jsonwebtoken::decode_header(token).context("malformed id_token")?;
    let key = match header.alg {
        // Authentik signs with the client secret (HS256) when the provider has no signing key.
        Algorithm::HS256 | Algorithm::HS384 | Algorithm::HS512 => {
            DecodingKey::from_secret(a.oidc_client_secret.as_bytes())
        }
        Algorithm::RS256
        | Algorithm::RS384
        | Algorithm::RS512
        | Algorithm::PS256
        | Algorithm::PS384
        | Algorithm::PS512
        | Algorithm::ES256
        | Algorithm::ES384 => {
            let jwks: JwkSet = st.http.get(&meta.jwks_uri).send().await?.error_for_status()?.json().await?;
            let jwk = match &header.kid {
                Some(kid) => jwks.find(kid),
                None => jwks.keys.first(),
            }
            .ok_or_else(|| anyhow!("no matching signing key in the provider's JWKS"))?;
            DecodingKey::from_jwk(jwk)?
        }
        other => bail!("unsupported id_token algorithm {other:?}"),
    };
    let mut v = Validation::new(header.alg);
    v.set_audience(&[a.oidc_client_id.as_str()]);
    v.set_issuer(&[meta.issuer.as_str()]);
    v.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
    Ok(jsonwebtoken::decode::<IdClaims>(token, &key, &v).context("id_token failed validation")?.claims)
}

pub async fn callback(
    State(st): State<AppState>,
    Query(q): Query<CallbackQuery>,
    jar: PrivateCookieJar,
) -> Result<Response, AppError> {
    if let Some(err) = q.error {
        let desc = q.error_description.unwrap_or_default();
        bail_page(format!("The identity provider returned an error: {err} {desc}"))?;
    }
    let (Some(code), Some(state)) = (q.code, q.state) else {
        bail_page("Missing code or state in the callback.".into())?;
        unreachable!()
    };
    let Some(raw) = jar.get(LOGIN_COOKIE) else {
        bail_page("The login session expired or cookies are blocked. Please try again.".into())?;
        unreachable!()
    };
    let ls: LoginState = serde_json::from_str(raw.value()).context("corrupt login cookie")?;
    if ls.state != state {
        bail_page("State mismatch; the sign-in attempt was rejected.".into())?;
    }

    let a = if ls.test { Arc::new(draft_cfg(&st)?) } else { st.auth() };
    let meta = oidc_meta(&st, &a).await?;
    let redirect_uri = format!("{}/auth/callback", a.public_url);
    let resp = st
        .http
        .post(&meta.token_endpoint)
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", redirect_uri.as_str()),
            ("client_id", a.oidc_client_id.as_str()),
            ("client_secret", a.oidc_client_secret.as_str()),
            ("code_verifier", ls.verifier.as_str()),
        ])
        .send()
        .await
        .context("token request failed")?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(anyhow!("token endpoint returned {status}: {body}").into());
    }
    let tokens: TokenResponse = resp.json().await.context("unexpected token response")?;
    let claims = verify_id_token(&st, &a, &meta, &tokens.id_token).await?;
    if claims.nonce.as_deref() != Some(ls.nonce.as_str()) {
        bail_page("Nonce mismatch; the sign-in attempt was rejected.".into())?;
    }

    // Groups usually ride in the ID token; fall back to userinfo.
    let mut extra = claims.extra.clone();
    let mut groups = parse_groups(extra.get(&a.groups_claim));
    let mut name = claims.name.clone().or(claims.preferred_username.clone());
    let mut email = claims.email.clone();
    if groups.is_empty() || name.is_none() {
        if let (Some(url), Some(at)) = (&meta.userinfo_endpoint, &tokens.access_token) {
            if let Ok(r) = st.http.get(url).bearer_auth(at).send().await {
                if let Ok(info) = r.json::<HashMap<String, Value>>().await {
                    extra.extend(info.clone());
                    if groups.is_empty() {
                        groups = parse_groups(info.get(&a.groups_claim));
                    }
                    name = name.or_else(|| info.get("name").and_then(Value::as_str).map(str::to_string));
                    name = name.or_else(|| info.get("preferred_username").and_then(Value::as_str).map(str::to_string));
                    email = email.or_else(|| info.get("email").and_then(Value::as_str).map(str::to_string));
                }
            }
        }
    }

    let jar = jar.remove(Cookie::build(LOGIN_COOKIE).path("/auth").build());
    if ls.test {
        let admin = evaluate(&a, &groups);
        let draft = st.settings.lock().unwrap().draft.clone().unwrap_or_default();
        let result = TestResult {
            fingerprint: fingerprint(&draft),
            sub: claims.sub.clone(),
            name: name.clone().unwrap_or_default(),
            groups: groups.clone(),
            admin: admin == Some(true),
            at: now(),
        };
        let ok = result.admin;
        let _ = st.update_settings(|s| s.tested = Some(result));
        let msg = match admin {
            Some(true) => format!("Signed in as {} (admin). Groups seen: {}. You can close this tab and press Enable in Settings.", name.unwrap_or_default(), if groups.is_empty() { "none".into() } else { groups.join(", ") }),
            Some(false) => format!("Signed in as {}, but that account would NOT be an administrator under these settings (groups seen: {}). Fix the admin group or the groups claim, then test again.", name.unwrap_or_default(), if groups.is_empty() { "none".into() } else { groups.join(", ") }),
            None => format!("Signed in as {}, but that account would be refused (groups seen: {}). Check the allowed/admin groups.", name.unwrap_or_default(), if groups.is_empty() { "none".into() } else { groups.join(", ") }),
        };
        return Ok((jar, page(if ok { "Test sign-in worked" } else { "Test sign-in needs attention" }, &msg, false)).into_response());
    }
    let Some(admin) = evaluate(&a, &groups) else {
        tracing::info!(user = %claims.sub, "login refused: not in an allowed group");
        return Ok((
            StatusCode::FORBIDDEN,
            jar,
            page(
                "Access denied",
                "Your account is not in a group that may use CraftHub. Ask an administrator to add you.",
                false,
            ),
        )
            .into_response());
    };

    let user = SessionUser {
        sub: claims.sub,
        name: name.unwrap_or_else(|| "Signed-in user".into()),
        email: email.unwrap_or_default(),
        admin,
        exp: now() + a.session_hours * 3600,
    };
    let cookie = short_cookie(
        SESSION_COOKIE,
        serde_json::to_string(&user)?,
        "/",
        a.cookie_secure,
        (a.session_hours * 3600) as i64,
    );
    Ok((jar.add(cookie), Redirect::to(&ls.next)).into_response())
}

fn bail_page(msg: String) -> Result<(), AppError> {
    Err(AppError(anyhow!(msg)))
}

pub async fn logout(State(st): State<AppState>, jar: PrivateCookieJar) -> Result<Response, AppError> {
    let jar = jar.remove(Cookie::build(SESSION_COOKIE).path("/").build());
    let a = st.auth();
    if a.mode == AuthMode::Oidc {
        if let Ok(meta) = oidc_meta(&st, &a).await {
            if let Some(end) = &meta.end_session_endpoint {
                return Ok((jar, Redirect::to(end)).into_response());
            }
        }
    }
    Ok((
        jar,
        Html(
            r#"<!doctype html><meta charset="utf-8"><title>Signed out</title>
<body style="font:16px system-ui;background:#0f1115;color:#e6e8ee;display:grid;place-items:center;min-height:100vh;margin:0">
<main><h1>Signed out</h1><p><a style="color:#5aa2ff" href="/auth/login">Sign in again</a></p></main>"#
                .to_string(),
        ),
    )
        .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(allowed: &[&str], admin: &[&str]) -> AuthCfg {
        AuthCfg {
            mode: AuthMode::Oidc,
            public_url: String::new(),
            cookie_secure: false,
            oidc_issuer: String::new(),
            oidc_client_id: String::new(),
            oidc_client_secret: String::new(),
            oidc_scopes: String::new(),
            groups_claim: "groups".into(),
            allowed_groups: allowed.iter().map(|s| s.to_string()).collect(),
            admin_groups: admin.iter().map(|s| s.to_string()).collect(),
            header_user: String::new(),
            header_email: String::new(),
            header_groups: String::new(),
            session_hours: 1,
        }
    }

    fn g(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn open_by_default_everyone_admin() {
        assert_eq!(evaluate(&cfg(&[], &[]), &g(&[])), Some(true));
    }

    #[test]
    fn admin_group_splits_roles() {
        let c = cfg(&[], &["craft-admins"]);
        assert_eq!(evaluate(&c, &g(&["craft-admins"])), Some(true));
        assert_eq!(evaluate(&c, &g(&["other"])), Some(false));
    }

    #[test]
    fn allowed_groups_gate_access() {
        let c = cfg(&["craft-users"], &["craft-admins"]);
        assert_eq!(evaluate(&c, &g(&["craft-users"])), Some(false));
        assert_eq!(evaluate(&c, &g(&["craft-admins"])), Some(true));
        assert_eq!(evaluate(&c, &g(&["nobody"])), None);
    }

    #[test]
    fn parses_group_shapes() {
        assert_eq!(parse_groups(Some(&serde_json::json!(["a", "b"]))), g(&["a", "b"]));
        assert_eq!(parse_groups(Some(&Value::String("a|b, c".into()))), g(&["a", "b", "c"]));
        assert!(parse_groups(None).is_empty());
    }

    #[test]
    fn next_param_is_sanitised() {
        assert_eq!(safe_next(Some("/apps/photocraft/")), "/apps/photocraft/");
        assert_eq!(safe_next(Some("//evil.example")), "/");
        assert_eq!(safe_next(Some("https://evil.example")), "/");
        assert_eq!(safe_next(None), "/");
    }
}
