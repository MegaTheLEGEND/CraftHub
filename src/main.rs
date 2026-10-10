mod api;
mod auth;
mod config;
mod github;
mod hosting;
mod manager;
mod state;
mod build_info;

use anyhow::{Context, Result};
use axum::{
    http::{header, HeaderValue},
    middleware,
    response::{Html, IntoResponse},
    routing::{delete, get, post},
    Router,
};
use axum_extra::extract::cookie::Key;
use config::{AuthMode, Config};
use state::{AppState, Shared, Store};
use std::{collections::HashMap, sync::Mutex as StdMutex};
use tokio::sync::Mutex;
use tower_http::{catch_panic::CatchPanicLayer, set_header::SetResponseHeaderLayer, trace::TraceLayer};
use tracing_subscriber::EnvFilter;

async fn ui_index() -> Html<&'static str> {
    Html(include_str!("../ui/index.html"))
}

fn build_http_client(cfg: &Config) -> Result<reqwest::Client> {
    let mut b = reqwest::Client::builder()
        .user_agent(concat!("craft-hub/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(std::time::Duration::from_secs(15))
        .timeout(std::time::Duration::from_secs(900));
    if let Some(path) = &cfg.extra_ca_file {
        let pem = std::fs::read(path).with_context(|| format!("reading EXTRA_CA_FILE {}", path.display()))?;
        b = b.add_root_certificate(reqwest::Certificate::from_pem(&pem).context("EXTRA_CA_FILE is not a PEM certificate")?);
    }
    Ok(b.build()?)
}

/// `craft-hub healthcheck`: used by the container HEALTHCHECK (the image has no curl).
async fn healthcheck() -> Result<()> {
    let bind = std::env::var("BIND").unwrap_or_else(|_| "0.0.0.0:8080".into());
    let port = bind.rsplit(':').next().unwrap_or("8080");
    let url = format!("http://127.0.0.1:{port}/healthz");
    let resp = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()?
        .get(url)
        .send()
        .await?;
    if resp.status().is_success() {
        Ok(())
    } else {
        anyhow::bail!("unhealthy: {}", resp.status())
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    if std::env::args().nth(1).as_deref() == Some("healthcheck") {
        return match healthcheck().await {
            Ok(()) => Ok(()),
            Err(e) => {
                eprintln!("{e:#}");
                std::process::exit(1);
            }
        };
    }

    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "craft_hub=info,tower_http=warn".into()))
        .init();

    let cfg = Config::from_env()?;
    std::fs::create_dir_all(cfg.data_dir.join("apps")).with_context(|| format!("creating {}", cfg.data_dir.display()))?;
    let catalog = config::load_catalog(&cfg.data_dir)?;
    let store = Store::load(&cfg.data_dir)?;
    let http = build_http_client(&cfg)?;

    let secret = state::session_secret(&cfg.data_dir, cfg.session_secret.as_deref())?;
    let key = {
        use sha2::{Digest, Sha512};
        Key::from(Sha512::digest(secret.as_bytes()).as_slice())
    };
    let settings = state::load_settings(&cfg.data_dir);
    let auth = if cfg.auth_recovery {
        tracing::warn!("AUTH_RECOVERY is on: saved authentication is ignored and auth is OFF. Remove it once you have reconfigured.");
        config::AuthCfg::open(&cfg)
    } else if let Some(saved) = &settings.auth {
        config::AuthCfg::from_saved(&cfg, saved)
    } else {
        config::AuthCfg::from_config(&cfg)
    };

    match auth.mode {
        AuthMode::None => tracing::warn!("authentication is OFF: anyone who can reach this port is an administrator. Turn on SSO in Settings."),
        AuthMode::Headers => tracing::warn!(
            "auth mode headers: only expose this container to your reverse proxy; the identity headers are trusted blindly"
        ),
        AuthMode::Oidc => {
            tracing::info!("OIDC issuer: {}", auth.oidc_issuer);
            tracing::info!("redirect URI to register: {}/auth/callback", auth.public_url);
            if auth.admin_groups.is_empty() {
                tracing::warn!("no admin groups configured: every signed-in user can install and remove versions");
            }
        }
    }

    let bind = cfg.bind.clone();
    let state = AppState(std::sync::Arc::new(Shared {
        cfg,
        catalog: std::sync::RwLock::new(catalog),
        store,
        http,
        jobs: StdMutex::new(HashMap::new()),
        releases: Mutex::new(HashMap::new()),
        key,
        settings: StdMutex::new(settings),
        auth: std::sync::RwLock::new(std::sync::Arc::new(auth)),
        oidc: Mutex::new(None),
    }));

    tokio::spawn(manager::updater(state.clone()));

    let protected = Router::new()
        .route("/", get(ui_index))
        .route("/api/me", get(api::me))
        .route("/api/apps", get(api::list_apps))
        .route("/api/catalog", post(api::add_app))
        .route("/api/catalog/{id}", delete(api::remove_app))
        .route("/api/check", post(api::check_all))
        .route("/api/settings", get(api::get_settings).post(api::set_settings))
        .route("/api/auth", get(api::get_auth))
        .route("/api/auth/draft", post(api::save_auth_draft))
        .route("/api/auth/enable", post(api::enable_auth))
        .route("/api/auth/disable", post(api::disable_auth))
        .route("/auth/test", get(auth::login_test))
        .route("/api/apps/{id}/refresh", post(api::refresh))
        .route("/api/apps/{id}/icon", get(api::icon))
        .route("/api/apps/{id}/releases", get(api::releases))
        .route("/api/apps/{id}/install", post(api::install))
        .route("/api/apps/{id}/activate", post(api::activate))
        .route("/api/apps/{id}/settings", post(api::settings))
        .route("/api/apps/{id}/versions/{version}", delete(api::remove_version))
        .route("/apps/{id}", get(hosting::redirect_to_slash))
        .route("/apps/{id}/", get(hosting::serve_app_root))
        .route("/apps/{id}/{*rest}", get(hosting::serve_app_path))
        .route("/versions/{id}/{version}/", get(hosting::serve_version_root))
        .route("/versions/{id}/{version}/{*rest}", get(hosting::serve_version_path))
        .layer(middleware::from_fn_with_state(state.clone(), auth::require_auth));

    let public = Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/auth/login", get(auth::login))
        .route("/auth/callback", get(auth::callback))
        .route("/auth/logout", get(auth::logout));

    let app = public
        .merge(protected)
        .with_state(state)
        .layer(SetResponseHeaderLayer::if_not_present(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::REFERRER_POLICY,
            HeaderValue::from_static("same-origin"),
        ))
        .layer(TraceLayer::new_for_http())
        .layer(CatchPanicLayer::custom(|e: Box<dyn std::any::Any + Send>| {
            let msg = e
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_else(|| "unknown panic".into());
            tracing::error!("handler panicked: {msg}");
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("Internal error: {msg}"),
            )
                .into_response()
        }));

    let listener = tokio::net::TcpListener::bind(&bind).await.with_context(|| format!("binding {bind}"))?;
    tracing::info!("craft-hub listening on {bind}");
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
