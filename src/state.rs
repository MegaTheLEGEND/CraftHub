use crate::config::{AppDef, AuthCfg, Config, SavedAuth};
use crate::github::Release;
use anyhow::{Context, Result};
use axum::extract::FromRef;
use axum_extra::extract::cookie::Key;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    ops::Deref,
    path::PathBuf,
    sync::{Arc, Mutex as StdMutex, RwLock},
    time::{Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::Mutex;

pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Persisted state (state.json)
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Installed {
    pub version: String,
    pub tag: String,
    pub installed_at: u64,
    pub size: u64,
    /// True when the download matched a published SHA-256.
    pub verified: bool,
    pub prerelease: bool,
}

fn yes() -> bool {
    true
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AppRecord {
    pub active: Option<String>,
    pub installed: Vec<Installed>,
    #[serde(default = "yes")]
    pub auto_update: bool,
    /// None = follow the global INCLUDE_PRERELEASE default.
    #[serde(default)]
    pub include_prerelease: Option<bool>,
    #[serde(default)]
    pub last_check: Option<u64>,
    #[serde(default)]
    pub last_error: Option<String>,
}

impl Default for AppRecord {
    fn default() -> Self {
        Self {
            active: None,
            installed: vec![],
            auto_update: true,
            include_prerelease: None,
            last_check: None,
            last_error: None,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Default, Debug)]
pub struct Persisted {
    #[serde(default)]
    pub apps: HashMap<String, AppRecord>,
}

pub struct Store {
    path: PathBuf,
    inner: Mutex<Persisted>,
}

impl Store {
    pub fn load(data_dir: &std::path::Path) -> Result<Self> {
        let path = data_dir.join("state.json");
        let inner = if path.exists() {
            let text = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
            serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?
        } else {
            Persisted::default()
        };
        Ok(Self { path, inner: Mutex::new(inner) })
    }

    pub async fn snapshot(&self) -> Persisted {
        self.inner.lock().await.clone()
    }

    pub async fn record(&self, id: &str) -> AppRecord {
        self.inner.lock().await.apps.get(id).cloned().unwrap_or_default()
    }

    /// Mutate one app's record and persist atomically.
    pub async fn update<R>(&self, id: &str, f: impl FnOnce(&mut AppRecord) -> R) -> Result<R> {
        let mut guard = self.inner.lock().await;
        let rec = guard.apps.entry(id.to_string()).or_default();
        let out = f(rec);
        let text = serde_json::to_string_pretty(&*guard)?;
        let tmp = self.path.with_extension("json.tmp");
        tokio::fs::write(&tmp, text).await?;
        tokio::fs::rename(&tmp, &self.path).await?;
        Ok(out)
    }
}

// ---------------------------------------------------------------------------
// In-memory state
// ---------------------------------------------------------------------------

#[derive(Serialize, Clone, Debug, Default)]
pub struct Job {
    /// downloading | extracting | done | error
    pub state: String,
    pub tag: String,
    pub message: String,
    pub done: u64,
    pub total: u64,
    pub updated_at: u64,
}

impl Job {
    pub fn running(&self) -> bool {
        self.state == "downloading" || self.state == "extracting"
    }
}

pub struct CachedReleases {
    pub fetched: Instant,
    pub list: Vec<Release>,
}

pub struct OidcMeta {
    pub issuer: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub jwks_uri: String,
    pub userinfo_endpoint: Option<String>,
    pub end_session_endpoint: Option<String>,
}

pub struct Shared {
    pub cfg: Config,
    pub catalog: Vec<AppDef>,
    pub store: Store,
    pub http: reqwest::Client,
    pub jobs: StdMutex<HashMap<String, Job>>,
    pub releases: Mutex<HashMap<String, CachedReleases>>,
    pub key: Key,
    /// Settings saved from the web UI (settings.json): GitHub token, active auth, auth draft.
    pub settings: StdMutex<PersistedSettings>,
    /// Effective authentication configuration (swapped when auth is enabled/disabled from the UI).
    pub auth: RwLock<Arc<AuthCfg>>,
    /// (fetched at, discovery issuer URL it was fetched for, metadata)
    pub oidc: Mutex<Option<(Instant, String, Arc<OidcMeta>)>>,
}

#[derive(Clone)]
pub struct AppState(pub Arc<Shared>);

impl Deref for AppState {
    type Target = Shared;
    fn deref(&self) -> &Shared {
        &self.0
    }
}

impl FromRef<AppState> for Key {
    fn from_ref(state: &AppState) -> Key {
        state.0.key.clone()
    }
}

impl Shared {
    pub fn app_def(&self, id: &str) -> Option<&AppDef> {
        self.catalog.iter().find(|a| a.id == id)
    }

    pub fn auth(&self) -> Arc<AuthCfg> {
        self.auth.read().unwrap().clone()
    }

    pub fn set_auth(&self, a: AuthCfg) {
        *self.auth.write().unwrap() = Arc::new(a);
    }

    pub fn github_token(&self) -> Option<String> {
        self.settings.lock().unwrap().github_token.clone().or_else(|| self.cfg.github_token.clone())
    }

    /// "ui", "env" or "none"
    pub fn token_source(&self) -> &'static str {
        if self.settings.lock().unwrap().github_token.is_some() {
            "ui"
        } else if self.cfg.github_token.is_some() {
            "env"
        } else {
            "none"
        }
    }

    /// Mutate the UI-saved settings and persist them (mode 600).
    pub fn update_settings<R>(&self, f: impl FnOnce(&mut PersistedSettings) -> R) -> Result<R> {
        let mut g = self.settings.lock().unwrap();
        let mut next = g.clone();
        let out = f(&mut next);
        write_private(&self.cfg.data_dir.join("settings.json"), &serde_json::to_string_pretty(&next)?)?;
        *g = next;
        Ok(out)
    }

    pub fn set_ui_token(&self, token: Option<String>) -> Result<()> {
        self.update_settings(|s| s.github_token = token)
    }

    pub fn apps_dir(&self) -> PathBuf {
        self.cfg.data_dir.join("apps")
    }

    pub fn job(&self, id: &str) -> Option<Job> {
        self.jobs.lock().unwrap().get(id).cloned()
    }

    pub fn job_running(&self, id: &str) -> bool {
        self.job(id).map(|j| j.running()).unwrap_or(false)
    }

    pub fn set_job(&self, id: &str, f: impl FnOnce(&mut Job)) {
        let mut jobs = self.jobs.lock().unwrap();
        let job = jobs.entry(id.to_string()).or_default();
        f(job);
        job.updated_at = now();
    }
}

/// Write a secret file readable only by the service user.
pub fn write_private(path: &std::path::Path, text: &str) -> Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, text)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct TestResult {
    /// Fingerprint of the draft that was tested; enabling needs it to match the current draft.
    pub fingerprint: String,
    pub sub: String,
    pub name: String,
    pub groups: Vec<String>,
    pub admin: bool,
    pub at: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct PersistedSettings {
    #[serde(default)]
    pub github_token: Option<String>,
    /// Authentication chosen in the UI; overrides the environment when present.
    #[serde(default)]
    pub auth: Option<SavedAuth>,
    /// Work in progress, not active until a successful test sign-in and "Enable".
    #[serde(default)]
    pub draft: Option<SavedAuth>,
    #[serde(default)]
    pub tested: Option<TestResult>,
}

pub fn fingerprint(a: &SavedAuth) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(serde_json::to_string(a).unwrap_or_default().as_bytes()))
}

pub fn load_settings(data_dir: &std::path::Path) -> PersistedSettings {
    std::fs::read_to_string(data_dir.join("settings.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .map(|mut s: PersistedSettings| {
            s.github_token = s.github_token.filter(|t| !t.is_empty());
            s
        })
        .unwrap_or_default()
}

/// Session secret: SESSION_SECRET if given, otherwise a random one generated once and kept in the data volume.
pub fn session_secret(data_dir: &std::path::Path, configured: Option<&str>) -> Result<String> {
    if let Some(s) = configured {
        return Ok(s.to_string());
    }
    let path = data_dir.join("session.key");
    if let Ok(s) = std::fs::read_to_string(&path) {
        if s.trim().len() >= 32 {
            return Ok(s.trim().to_string());
        }
    }
    use rand::RngCore;
    let mut buf = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut buf);
    let secret = hex::encode(buf);
    write_private(&path, &secret)?;
    Ok(secret)
}
