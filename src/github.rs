use crate::config::AppDef;
use crate::state::{CachedReleases, Shared};
use anyhow::{anyhow, bail, Result};
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

const CACHE_TTL: Duration = Duration::from_secs(600);

#[derive(Clone, Debug, Serialize)]
pub struct WebAsset {
    pub name: String,
    pub url: String,
    pub size: u64,
    pub sha256: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct NativeAsset {
    pub name: String,
    pub url: String,
    pub size: u64,
    /// windows | macos | linux | freebsd | other
    pub os: String,
    pub arch: String,
    /// installer | package | portable | appimage | archive | cli
    pub kind: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct Release {
    pub tag: String,
    pub version: String,
    pub name: String,
    pub prerelease: bool,
    pub published_at: Option<String>,
    pub url: String,
    pub web: Option<WebAsset>,
    pub native: Vec<NativeAsset>,
    pub sums_url: Option<String>,
}

#[derive(Deserialize)]
struct GhAsset {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    digest: Option<String>,
}

#[derive(Deserialize)]
struct GhRelease {
    tag_name: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    published_at: Option<String>,
    #[serde(default)]
    html_url: String,
    #[serde(default)]
    assets: Vec<GhAsset>,
}

fn classify_native(a: &GhAsset) -> Option<NativeAsset> {
    let n = a.name.to_ascii_lowercase();
    if n.ends_with(".txt") || n.ends_with(".sha256") || n.ends_with(".sig") || n.ends_with(".json") || n.ends_with(".asc") {
        return None;
    }
    let os = if n.contains("windows") || n.contains("-win") || n.ends_with(".msi") || n.ends_with(".exe") {
        "windows"
    } else if n.contains("macos") || n.contains("darwin") || n.ends_with(".dmg") {
        "macos"
    } else if n.contains("freebsd") {
        "freebsd"
    } else if n.contains("linux") || n.ends_with(".deb") || n.ends_with(".rpm") || n.ends_with(".appimage") || n.ends_with(".flatpak") {
        "linux"
    } else {
        return None;
    };
    let arch = if n.contains("universal") {
        "universal"
    } else if n.contains("aarch64") || n.contains("arm64") {
        "aarch64"
    } else if n.contains("x86_64") || n.contains("x64") || n.contains("amd64") {
        "x86_64"
    } else if n.contains("x86") {
        "x86"
    } else {
        "any"
    };
    let kind = if n.contains("-cli-") {
        "cli"
    } else if n.ends_with(".msi") || n.ends_with(".dmg") {
        "installer"
    } else if n.ends_with(".deb") || n.ends_with(".rpm") || n.ends_with(".flatpak") {
        "package"
    } else if n.ends_with(".appimage") {
        "appimage"
    } else if n.contains("portable") {
        "portable"
    } else {
        "archive"
    };
    Some(NativeAsset {
        name: a.name.clone(),
        url: a.browser_download_url.clone(),
        size: a.size,
        os: os.into(),
        arch: arch.into(),
        kind: kind.into(),
    })
}

fn to_release(r: GhRelease, web_marker: &str) -> Release {
    let marker = web_marker.to_ascii_lowercase();
    let web = r
        .assets
        .iter()
        .find(|a| {
            let n = a.name.to_ascii_lowercase();
            n.ends_with(".zip") && n.contains(&marker)
        })
        .map(|a| WebAsset {
            name: a.name.clone(),
            url: a.browser_download_url.clone(),
            size: a.size,
            sha256: a
                .digest
                .as_deref()
                .and_then(|d| d.strip_prefix("sha256:"))
                .map(|s| s.to_ascii_lowercase()),
        });
    let sums_url = r
        .assets
        .iter()
        .find(|a| a.name.eq_ignore_ascii_case("SHA256SUMS.txt"))
        .map(|a| a.browser_download_url.clone());
    let web_name = web.as_ref().map(|w| w.name.clone());
    let native = r
        .assets
        .iter()
        .filter(|a| Some(&a.name) != web_name.as_ref())
        .filter_map(classify_native)
        .collect();
    Release {
        version: r.tag_name.trim_start_matches('v').to_string(),
        name: r.name.filter(|n| !n.is_empty()).unwrap_or_else(|| r.tag_name.clone()),
        tag: r.tag_name,
        prerelease: r.prerelease,
        published_at: r.published_at,
        url: r.html_url,
        web,
        native,
        sums_url,
    }
}

/// Newest release (GitHub returns newest first) that ships a web build.
pub fn pick_latest(list: &[Release], include_prerelease: bool) -> Option<&Release> {
    list.iter().find(|r| r.web.is_some() && (include_prerelease || !r.prerelease))
}

impl Shared {
    async fn fetch_releases(&self, def: &AppDef) -> Result<Vec<Release>> {
        let url = format!(
            "{}/repos/{}/releases?per_page=30",
            self.cfg.github_api.trim_end_matches('/'),
            def.repo
        );
        let mut rq = self
            .http
            .get(&url)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28");
        if let Some(t) = self.github_token() {
            rq = rq.bearer_auth(t);
        }
        let resp = rq.send().await.map_err(|e| anyhow!("GitHub request failed: {e}"))?;
        let status = resp.status();
        if status.as_u16() == 404 {
            return Ok(vec![]);
        }
        if status.as_u16() == 403 || status.as_u16() == 429 {
            bail!("GitHub API refused the request ({status}); you are probably rate limited, set GITHUB_TOKEN");
        }
        if !status.is_success() {
            bail!("GitHub API returned {status} for {}", def.repo);
        }
        let gh: Vec<GhRelease> = resp.json().await.map_err(|e| anyhow!("bad GitHub response: {e}"))?;
        Ok(gh
            .into_iter()
            .filter(|r| !r.draft)
            .map(|r| to_release(r, &def.web_asset_contains))
            .collect())
    }

    /// Releases for an app, cached for ten minutes unless `force`.
    pub async fn releases(&self, id: &str, force: bool) -> Result<Vec<Release>> {
        let def = self.app_def(id).ok_or_else(|| anyhow!("unknown app '{id}'"))?.clone();
        if !force {
            let cache = self.releases.lock().await;
            if let Some(c) = cache.get(id) {
                if c.fetched.elapsed() < CACHE_TTL {
                    return Ok(c.list.clone());
                }
            }
        }
        let list = self.fetch_releases(&def).await?;
        self.releases
            .lock()
            .await
            .insert(id.to_string(), CachedReleases { fetched: Instant::now(), list: list.clone() });
        Ok(list)
    }

    /// Whatever is cached right now, without touching the network.
    pub async fn cached_releases(&self, id: &str) -> Vec<Release> {
        self.releases.lock().await.get(id).map(|c| c.list.clone()).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asset(name: &str) -> GhAsset {
        GhAsset { name: name.into(), browser_download_url: format!("https://x/{name}"), size: 1, digest: None }
    }

    #[test]
    fn splits_web_and_native_assets() {
        let r = GhRelease {
            tag_name: "v0.1.1-rc.4".into(),
            name: None,
            draft: false,
            prerelease: true,
            published_at: None,
            html_url: String::new(),
            assets: vec![
                asset("photocraft-web-0.1.1-rc.4.zip"),
                asset("photocraft-0.1.1-rc.4-linux-x86_64.AppImage"),
                asset("photocraft-0.1.1-rc.4-windows-x64-portable.zip"),
                asset("photocraft-0.1.1-rc.4-macos-universal.dmg"),
                asset("photocraft-cli-0.1.1-rc.4-macos-universal.zip"),
                asset("SHA256SUMS.txt"),
            ],
        };
        let rel = to_release(r, "-web-");
        assert_eq!(rel.version, "0.1.1-rc.4");
        assert_eq!(rel.web.unwrap().name, "photocraft-web-0.1.1-rc.4.zip");
        assert_eq!(rel.native.len(), 4);
        assert!(rel.sums_url.is_some());
        assert!(rel.native.iter().any(|n| n.os == "macos" && n.kind == "installer"));
        assert!(rel.native.iter().any(|n| n.kind == "cli"));
    }
}
