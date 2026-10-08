use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, env, path::{Path, PathBuf}};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthMode {
    /// OpenID Connect authorization-code flow (Authentik, Keycloak, ...).
    Oidc,
    /// Trust identity headers set by a reverse proxy / Authentik proxy outpost.
    Headers,
    /// No authentication. Local development only.
    None,
}

#[derive(Clone)]
pub struct Config {
    pub bind: String,
    pub data_dir: PathBuf,
    pub public_url: String,
    pub cookie_secure: bool,
    pub auth_mode: AuthMode,
    /// AUTH_RECOVERY=true ignores authentication saved from the UI and runs with auth off.
    pub auth_recovery: bool,

    pub oidc_issuer: String,
    pub oidc_client_id: String,
    pub oidc_client_secret: String,
    pub oidc_scopes: String,
    pub groups_claim: String,
    pub allowed_groups: Vec<String>,
    pub admin_groups: Vec<String>,

    pub header_user: String,
    pub header_email: String,
    pub header_groups: String,

    pub session_secret: Option<String>,
    pub session_hours: u64,

    pub github_token: Option<String>,
    pub github_api: String,
    pub update_interval_minutes: u64,
    pub auto_install: bool,
    pub keep_versions: usize,
    pub include_prerelease: bool,
    pub extra_ca_file: Option<PathBuf>,
}

fn opt(key: &str) -> Option<String> {
    env::var(key).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

fn csv(key: &str) -> Vec<String> {
    opt(key)
        .map(|v| v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())
        .unwrap_or_default()
}

fn flag(key: &str, default: bool) -> bool {
    match opt(key).map(|v| v.to_ascii_lowercase()) {
        Some(v) => matches!(v.as_str(), "1" | "true" | "yes" | "on"),
        None => default,
    }
}

fn num<T: std::str::FromStr>(key: &str, default: T) -> Result<T> {
    match opt(key) {
        Some(v) => v.parse::<T>().map_err(|_| anyhow::anyhow!("{key} must be a number, got '{v}'")),
        None => Ok(default),
    }
}

impl Config {
    pub fn from_env() -> Result<Self> {
        // No AUTH_MODE: OIDC only if fully configured by env, otherwise start open and configure it in the UI.
        let env_oidc = opt("OIDC_ISSUER").is_some() && opt("OIDC_CLIENT_ID").is_some() && opt("OIDC_CLIENT_SECRET").is_some();
        let default_mode = if env_oidc { "oidc" } else { "none" };
        let auth_mode = match opt("AUTH_MODE").unwrap_or_else(|| default_mode.into()).to_ascii_lowercase().as_str() {
            "oidc" => AuthMode::Oidc,
            "headers" => AuthMode::Headers,
            "none" => AuthMode::None,
            other => bail!("AUTH_MODE must be oidc, headers or none (got '{other}')"),
        };
        let public_url = opt("PUBLIC_URL").unwrap_or_else(|| "http://localhost:8080".into());
        let public_url = public_url.trim_end_matches('/').to_string();

        let cfg = Config {
            bind: opt("BIND").unwrap_or_else(|| "0.0.0.0:8080".into()),
            data_dir: PathBuf::from(opt("DATA_DIR").unwrap_or_else(|| "/data".into())),
            cookie_secure: public_url.starts_with("https://"),
            public_url,
            auth_mode,
            auth_recovery: flag("AUTH_RECOVERY", false),
            oidc_issuer: opt("OIDC_ISSUER").unwrap_or_default(),
            oidc_client_id: opt("OIDC_CLIENT_ID").unwrap_or_default(),
            oidc_client_secret: opt("OIDC_CLIENT_SECRET").unwrap_or_default(),
            oidc_scopes: opt("OIDC_SCOPES").unwrap_or_else(|| "openid profile email".into()),
            groups_claim: opt("OIDC_GROUPS_CLAIM").unwrap_or_else(|| "groups".into()),
            allowed_groups: csv("ALLOWED_GROUPS"),
            admin_groups: csv("ADMIN_GROUPS"),
            header_user: opt("HEADER_USER").unwrap_or_else(|| "X-authentik-username".into()),
            header_email: opt("HEADER_EMAIL").unwrap_or_else(|| "X-authentik-email".into()),
            header_groups: opt("HEADER_GROUPS").unwrap_or_else(|| "X-authentik-groups".into()),
            session_secret: opt("SESSION_SECRET"),
            session_hours: num("SESSION_HOURS", 12u64)?,
            github_token: opt("GITHUB_TOKEN"),
            github_api: opt("GITHUB_API_URL").unwrap_or_else(|| "https://api.github.com".into()),
            update_interval_minutes: match (opt("UPDATE_INTERVAL_MINUTES"), opt("UPDATE_INTERVAL_HOURS")) {
                (Some(_), _) => num("UPDATE_INTERVAL_MINUTES", 30u64)?,
                (None, Some(_)) => num("UPDATE_INTERVAL_HOURS", 1u64)?.saturating_mul(60),
                (None, None) => 30,
            },
            auto_install: flag("AUTO_INSTALL", true),
            keep_versions: num("KEEP_VERSIONS", 3usize)?.max(1),
            include_prerelease: flag("INCLUDE_PRERELEASE", true),
            extra_ca_file: opt("EXTRA_CA_FILE").map(PathBuf::from),
        };
        cfg.validate()?;
        Ok(cfg)
    }

    fn validate(&self) -> Result<()> {
        if self.auth_mode == AuthMode::Oidc {
            if self.oidc_issuer.is_empty() || self.oidc_client_id.is_empty() || self.oidc_client_secret.is_empty() {
                bail!("AUTH_MODE=oidc requires OIDC_ISSUER, OIDC_CLIENT_ID and OIDC_CLIENT_SECRET");
            }
            if let Some(s) = &self.session_secret {
                if s.len() < 32 {
                    bail!("SESSION_SECRET must be at least 32 characters (try: openssl rand -hex 32)");
                }
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Runtime authentication settings (switchable from the UI)
// ---------------------------------------------------------------------------

/// What the UI saves: `mode` is "none", "oidc" or "headers".
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SavedAuth {
    #[serde(default)]
    pub mode: String,
    #[serde(default)]
    pub public_url: String,
    #[serde(default)]
    pub oidc_issuer: String,
    #[serde(default)]
    pub oidc_client_id: String,
    #[serde(default)]
    pub oidc_client_secret: String,
    #[serde(default)]
    pub oidc_scopes: String,
    #[serde(default)]
    pub groups_claim: String,
    #[serde(default)]
    pub allowed_groups: Vec<String>,
    #[serde(default)]
    pub admin_groups: Vec<String>,
}

/// The effective authentication configuration; swapped at runtime.
#[derive(Clone, Debug)]
pub struct AuthCfg {
    pub mode: AuthMode,
    pub public_url: String,
    pub cookie_secure: bool,
    pub oidc_issuer: String,
    pub oidc_client_id: String,
    pub oidc_client_secret: String,
    pub oidc_scopes: String,
    pub groups_claim: String,
    pub allowed_groups: Vec<String>,
    pub admin_groups: Vec<String>,
    pub header_user: String,
    pub header_email: String,
    pub header_groups: String,
    pub session_hours: u64,
}

impl AuthCfg {
    pub fn from_config(c: &Config) -> Self {
        AuthCfg {
            mode: c.auth_mode.clone(),
            public_url: c.public_url.clone(),
            cookie_secure: c.cookie_secure,
            oidc_issuer: c.oidc_issuer.clone(),
            oidc_client_id: c.oidc_client_id.clone(),
            oidc_client_secret: c.oidc_client_secret.clone(),
            oidc_scopes: c.oidc_scopes.clone(),
            groups_claim: c.groups_claim.clone(),
            allowed_groups: c.allowed_groups.clone(),
            admin_groups: c.admin_groups.clone(),
            header_user: c.header_user.clone(),
            header_email: c.header_email.clone(),
            header_groups: c.header_groups.clone(),
            session_hours: c.session_hours,
        }
    }

    pub fn open(c: &Config) -> Self {
        AuthCfg { mode: AuthMode::None, ..Self::from_config(c) }
    }

    pub fn from_saved(c: &Config, s: &SavedAuth) -> Self {
        let mut a = Self::from_config(c);
        a.mode = match s.mode.as_str() {
            "oidc" => AuthMode::Oidc,
            "headers" => AuthMode::Headers,
            _ => AuthMode::None,
        };
        if !s.public_url.is_empty() {
            a.public_url = s.public_url.trim_end_matches('/').to_string();
        }
        a.cookie_secure = a.public_url.starts_with("https://");
        a.oidc_issuer = s.oidc_issuer.clone();
        a.oidc_client_id = s.oidc_client_id.clone();
        a.oidc_client_secret = s.oidc_client_secret.clone();
        a.oidc_scopes = if s.oidc_scopes.is_empty() { "openid profile email".into() } else { s.oidc_scopes.clone() };
        a.groups_claim = if s.groups_claim.is_empty() { "groups".into() } else { s.groups_claim.clone() };
        a.allowed_groups = s.allowed_groups.clone();
        a.admin_groups = s.admin_groups.clone();
        a
    }
}

// ---------------------------------------------------------------------------
// App catalog
// ---------------------------------------------------------------------------

pub fn is_safe_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 100
        && s != "."
        && s != ".."
        && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '+'))
}

fn default_marker() -> String {
    "-web-".into()
}

#[derive(Clone, Debug, Deserialize)]
pub struct AppDef {
    pub id: String,
    #[serde(default)]
    pub name: String,
    /// GitHub "owner/repo" that publishes releases.
    #[serde(default)]
    pub repo: String,
    #[serde(default)]
    pub description: String,
    /// Substring that identifies the web-build zip among release assets.
    #[serde(default = "default_marker")]
    pub web_asset_contains: String,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub disabled: bool,
}

#[derive(Deserialize)]
struct CatalogFile {
    #[serde(default)]
    app: Vec<AppDef>,
}

const BUILTIN: &[(&str, &str, &str)] = &[
    ("photocraft", "PhotoCraft", "Replaces Photoshop. Image editing: layers, masks, type and real PSD files"),
    ("vectorcraft", "VectorCraft", "Replaces Illustrator. Vector illustration"),
    ("filmcraft", "FilmCraft", "Replaces Premiere Pro. Video editing, color and sound"),
    ("lightcraft", "LightCraft", "Replaces Lightroom. Photo library and raw development"),
    ("pdfcraft", "PdfCraft / PrintCraft", "Replaces Acrobat. Reading, organizing and protecting PDFs"),
    ("effectcraft", "EffectCraft", "Replaces After Effects. Motion graphics and visual effects"),
    ("designcraft", "DesignCraft", "Replaces InDesign. Page layout and publishing"),
    ("cadcraft", "CADCraft", "Replaces AutoCAD. Computer-aided design and drafting"),
    ("wordcraft", "WordCraft", "Replaces Microsoft Word. Word processing"),
    ("gridcraft", "GridCraft", "Replaces Microsoft Excel. Spreadsheets"),
    ("soundcraft", "SoundCraft", "Replaces Pro Tools. Audio production"),
    ("deckcraft", "DeckCraft", "Replaces PowerPoint. Presentations and slide shows"),
];

pub fn load_catalog(data_dir: &Path) -> Result<Vec<AppDef>> {
    let mut apps: Vec<AppDef> = BUILTIN
        .iter()
        .map(|(id, name, desc)| AppDef {
            id: (*id).into(),
            name: (*name).into(),
            repo: format!("storytold/{id}"),
            description: (*desc).into(),
            web_asset_contains: default_marker(),
            icon: None,
            disabled: false,
        })
        .collect();

    let path = data_dir.join("apps.toml");
    if path.exists() {
        let text = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        let file: CatalogFile = toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        let mut by_id: HashMap<String, usize> = apps.iter().enumerate().map(|(i, a)| (a.id.clone(), i)).collect();
        for mut def in file.app {
            if !is_safe_name(&def.id) {
                bail!("apps.toml: invalid app id '{}'", def.id);
            }
            if def.name.is_empty() {
                def.name = def.id.clone();
            }
            if def.repo.is_empty() {
                def.repo = format!("storytold/{}", def.id);
            }
            match by_id.get(&def.id) {
                Some(&i) => apps[i] = def,
                None => {
                    by_id.insert(def.id.clone(), apps.len());
                    apps.push(def);
                }
            }
        }
    }
    apps.retain(|a| !a.disabled);
    Ok(apps)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_names() {
        assert!(is_safe_name("0.1.1-rc.4"));
        assert!(is_safe_name("photocraft"));
        assert!(!is_safe_name(".."));
        assert!(!is_safe_name("../etc"));
        assert!(!is_safe_name("a/b"));
        assert!(!is_safe_name(""));
    }

    #[test]
    fn builtin_catalog_has_photocraft() {
        let dir = std::env::temp_dir().join("craft-hub-test-nonexistent");
        let apps = load_catalog(&dir).unwrap();
        assert!(apps.iter().any(|a| a.id == "photocraft" && a.repo == "storytold/photocraft"));
    }
}
