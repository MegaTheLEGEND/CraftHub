//! Releases: GitHub (full lists, conditional requests) and ArtCraft's release feed (latest only).
//! Parsing, URL allow-listing, checksum parsing and asset choice come from `artcraft-launcher-core`.

use crate::config::AppDef;
use crate::state::{CachedReleases, Shared, now};
use anyhow::{Result, anyhow, bail};
use artcraft_launcher_core::{
    Arch, CraftApp, Os, PackageKind, Platform, feed::{Feed, FeedAnswer}, platform, release, version,
};
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

const CACHE_TTL: Duration = Duration::from_secs(600);
const FEED_TTL: Duration = Duration::from_secs(120);

#[derive(Clone, Debug, Serialize)]
pub struct WebAsset {
    pub name: String,
    pub url: String,
    pub size: u64,
    pub sha256: Option<String>,
}

/// A desktop download.
#[derive(Clone, Debug, Serialize)]
pub struct Native {
    pub name: String,
    pub url: String,
    pub size: u64,
    /// windows | macos | linux | freebsd
    pub os: String,
    /// x64 | x86 | arm64 | universal | x86_64 | aarch64 | riscv64 | any
    pub arch: String,
    /// msi | portable | dmg | appimage | deb | rpm | flatpak | tarball | cli
    pub kind: String,
    /// English label of the package format (translated by the UI).
    pub label: String,
}

#[derive(Clone, Debug)]
pub struct Release {
    pub tag: String,
    /// The tag without a leading `v`: the folder name of an installed version.
    pub version: String,
    pub semver: Option<version::Version>,
    pub name: String,
    pub prerelease: bool,
    pub published_at: Option<i64>,
    pub url: String,
    pub notes: String,
    pub web: Option<WebAsset>,
    pub sums_url: Option<String>,
    pub native: Vec<Native>,
}

fn kind_key(k: PackageKind) -> &'static str {
    match k {
        PackageKind::Msi => "msi",
        PackageKind::PortableZip => "portable",
        PackageKind::Dmg => "dmg",
        PackageKind::AppImage => "appimage",
        PackageKind::Deb => "deb",
        PackageKind::Rpm => "rpm",
        PackageKind::Flatpak => "flatpak",
        PackageKind::TarGz => "tarball",
    }
}

/// Asset-name stems an app's desktop files may use.
pub fn stems_for(def: &AppDef) -> Vec<String> {
    let mut v = vec![def.id.clone()];
    if let Some(a) = CraftApp::by_id(&def.id) {
        v.extend(a.former_ids.iter().map(|s| s.to_string()));
    }
    if let Some((_, name)) = def.repo.split_once('/') {
        let n = name.to_ascii_lowercase();
        if !v.contains(&n) {
            v.push(n);
        }
    }
    v
}

fn os_of(token: &str) -> Option<&'static str> {
    ["windows", "macos", "linux", "freebsd"].into_iter().find(|o| token.contains(o))
}

fn classify(a: &release::Asset, stems: &[&str]) -> Option<Native> {
    if let Some(p) = platform::parse_asset_name(&a.name, stems) {
        let (os, arch) = p.token.split_once('-')?;
        return Some(Native {
            name: a.name.clone(),
            url: a.url.clone(),
            size: a.size,
            os: os.into(),
            arch: arch.into(),
            kind: kind_key(p.kind).into(),
            label: p.kind.label().into(),
        });
    }
    // The command-line builds the launcher itself ignores are still worth listing.
    let n = a.name.to_ascii_lowercase();
    if n.contains("-cli-") && (n.ends_with(".zip") || n.ends_with(".tar.gz")) {
        let os = os_of(&n)?;
        let arch = ["universal", "aarch64", "arm64", "x86_64", "x64", "riscv64", "x86"].into_iter().find(|x| n.contains(x)).unwrap_or("any");
        return Some(Native { name: a.name.clone(), url: a.url.clone(), size: a.size, os: os.into(), arch: arch.into(), kind: "cli".into(), label: "Command line".into() });
    }
    None
}

fn find_web<'a>(assets: &'a [release::Asset], marker: &str) -> Option<&'a release::Asset> {
    assets.iter().find(|a| {
        let n = a.name.to_ascii_lowercase();
        n.ends_with(".zip") && n.contains(marker)
    })
}

pub fn build_release(r: release::Release, def: &AppDef) -> Release {
    let marker = def.web_asset_contains.to_ascii_lowercase();
    // The app's own marker first; the standard "-web-" as a fallback, so an entry saved with an old
    // name (`solvecraft-web`) still finds `solidcraft-web-0.14.0.zip` after a repository rename.
    let web = find_web(&r.assets, &marker).or_else(|| find_web(&r.assets, "-web-"));
    let sums_url = r.sums_asset().map(|a| a.url.clone());
    let stems = stems_for(def);
    let stems: Vec<&str> = stems.iter().map(String::as_str).collect();
    let native = r
        .assets
        .iter()
        .filter(|a| web.map(|w| w.name != a.name).unwrap_or(true))
        .filter_map(|a| classify(a, &stems))
        .collect();
    Release {
        version: r.tag.trim_start_matches('v').to_string(),
        semver: r.version.clone(),
        name: r.name.clone(),
        prerelease: r.prerelease,
        published_at: r.published_at,
        url: r.html_url.clone(),
        notes: r.notes.clone(),
        web: web.map(|a| WebAsset { name: a.name.clone(), url: a.url.clone(), size: a.size, sha256: a.sha256.clone() }),
        sums_url,
        native,
        tag: r.tag,
    }
}

/// Newest release (by version, not by list order) that ships a web build.
pub fn pick_latest(list: &[Release], include_prerelease: bool) -> Option<&Release> {
    let mut best: Option<&Release> = None;
    for r in list.iter().filter(|r| r.web.is_some() && (include_prerelease || !r.prerelease)) {
        best = match best {
            None => Some(r),
            Some(b) => match (&r.semver, &b.semver) {
                (Some(x), Some(y)) if version::is_newer(x, y) => Some(r),
                _ => Some(b),
            },
        };
    }
    best
}

/// Releases newest first: by version where both parse, otherwise as GitHub listed them.
pub fn sort_newest_first(list: &mut [Release]) {
    list.sort_by(|a, b| match (&a.semver, &b.semver) {
        (Some(x), Some(y)) => version::compare(y, x),
        _ => std::cmp::Ordering::Equal,
    });
}

/// Where each visitor platform should download from: `windows-x64`, `macos`, `linux-arm64`, ...
pub fn downloads_for(rel: &Release, stems: &[&str]) -> BTreeMap<&'static str, Native> {
    // Rebuild the core view of the release's desktop files so its asset choice can run on them.
    let core = release::Release {
        assets: rel
            .native
            .iter()
            .map(|n| release::Asset { name: n.name.clone(), size: n.size, url: n.url.clone(), sha256: None })
            .collect(),
        ..Default::default()
    };
    let plats: [(&str, Os, Arch); 8] = [
        ("windows-x64", Os::Windows, Arch::X64),
        ("windows-arm64", Os::Windows, Arch::Arm64),
        ("windows-x86", Os::Windows, Arch::X86),
        ("macos", Os::MacOs, Arch::Arm64),
        ("linux-x64", Os::Linux, Arch::X64),
        ("linux-arm64", Os::Linux, Arch::Arm64),
        ("linux-riscv64", Os::Linux, Arch::Riscv64),
        ("freebsd-x64", Os::FreeBsd, Arch::X64),
    ];
    let mut out = BTreeMap::new();
    for (key, os, arch) in plats {
        let p = Platform { os, arch };
        if let Some((asset, _)) = platform::pick_asset(&core, stems, p, p.default_kinds()) {
            if let Some(n) = rel.native.iter().find(|n| n.name == asset.name) {
                out.insert(key, n.clone());
            }
        }
    }
    out
}

/// The repository name GitHub says this release belongs to, if it is a same-owner rename of `repo`
/// (e.g. solvecraft -> solidcraft). Download links name the new repository.
fn canonical_repo(item: &Value, repo: &str) -> String {
    let owner = repo.split('/').next().unwrap_or("");
    let url = item["html_url"].as_str().unwrap_or("");
    if let Some(rest) = url.strip_prefix("https://github.com/") {
        let mut it = rest.split('/');
        if let (Some(o), Some(n), Some("releases")) = (it.next(), it.next(), it.next()) {
            if o.eq_ignore_ascii_case(owner) && !n.is_empty() {
                return format!("{o}/{n}");
            }
        }
    }
    repo.to_string()
}

pub struct RepoInfo {
    pub full_name: String,
    pub name: String,
    pub description: String,
}

enum Fetched {
    NotModified,
    Fresh(Vec<Release>, Option<String>),
}

impl Shared {
    /// The repository to ask GitHub about: the feed may say a suite app moved within ArtCraft's account.
    pub fn repo_for(&self, def: &AppDef) -> String {
        if let Some(app) = CraftApp::by_id(&def.id) {
            if def.repo.eq_ignore_ascii_case(app.repo) {
                if let Some(f) = self.feed.lock().unwrap().as_ref() {
                    return f.1.repo_for(app);
                }
            }
        }
        def.repo.clone()
    }

    fn note_rate_limit(&self, resp: &reqwest::Response) {
        let reset = resp.headers().get("x-ratelimit-reset").and_then(|v| v.to_str().ok()).and_then(|v| v.parse::<u64>().ok());
        let until = reset.unwrap_or_else(|| now() + 600);
        *self.rate_limited_until.lock().unwrap() = Some(until);
    }

    async fn fetch_releases(&self, def: &AppDef, etag: Option<&str>) -> Result<Fetched> {
        let repo = self.repo_for(def);
        let url = format!("{}/repos/{}/releases?per_page=30", self.cfg.github_api.trim_end_matches('/'), repo);
        let mut rq = self.http.get(&url).header("Accept", "application/vnd.github+json").header("X-GitHub-Api-Version", "2022-11-28");
        if let Some(e) = etag {
            rq = rq.header("If-None-Match", e);
        }
        if let Some(t) = self.github_token() {
            rq = rq.bearer_auth(t);
        }
        let resp = rq.send().await.map_err(|e| anyhow!("GitHub request failed: {e}"))?;
        let status = resp.status();
        if resp.headers().get("x-ratelimit-remaining").and_then(|v| v.to_str().ok()) == Some("0") {
            tracing::warn!("GitHub API rate limit exhausted; add a GitHub token in Settings");
            self.note_rate_limit(&resp);
        }
        match status.as_u16() {
            304 => return Ok(Fetched::NotModified),
            404 => return Ok(Fetched::Fresh(vec![], None)),
            403 | 429 => {
                self.note_rate_limit(&resp);
                bail!("GitHub API refused the request ({status}); you are probably rate limited, add a GitHub token in Settings");
            }
            _ if !status.is_success() => bail!("GitHub API returned {status} for {repo}"),
            _ => {}
        }
        *self.rate_limited_until.lock().unwrap() = None;
        let new_etag = resp.headers().get("etag").and_then(|v| v.to_str().ok()).map(str::to_string);
        let items: Vec<Value> = resp.json().await.map_err(|e| anyhow!("bad GitHub response: {e}"))?;
        let mut list = Vec::new();
        for item in items.iter().filter(|i| !i["draft"].as_bool().unwrap_or(false)) {
            let r = canonical_repo(item, &repo);
            if let Ok(rel) = release::parse_github_release(&item.to_string(), &r) {
                list.push(build_release(rel, def));
            }
        }
        sort_newest_first(&mut list);
        Ok(Fetched::Fresh(list, new_etag))
    }

    /// Releases for an app, newest first. Cached ten minutes unless `force`, then re-checked with a
    /// conditional request. Runs on the server only: browsers never contact GitHub.
    pub async fn releases(&self, id: &str, force: bool) -> Result<Vec<Release>> {
        let def = self.app_def(id).ok_or_else(|| anyhow!("unknown app '{id}'"))?;
        let (prev_list, prev_etag) = {
            let cache = self.releases.lock().await;
            match cache.get(id) {
                Some(c) => {
                    if !force && c.fetched.elapsed() < CACHE_TTL {
                        return Ok(c.list.clone());
                    }
                    (Some(c.list.clone()), c.etag.clone())
                }
                None => (None, None),
            }
        };
        let (list, etag) = match self.fetch_releases(&def, prev_etag.as_deref().filter(|_| prev_list.is_some())).await? {
            Fetched::NotModified => (prev_list.unwrap_or_default(), prev_etag),
            Fetched::Fresh(list, etag) => (list, etag),
        };
        self.releases.lock().await.insert(id.to_string(), CachedReleases { fetched: Instant::now(), list: list.clone(), etag });
        Ok(list)
    }

    pub async fn repo_info(&self, repo: &str) -> Result<RepoInfo> {
        let url = format!("{}/repos/{}", self.cfg.github_api.trim_end_matches('/'), repo);
        let mut rq = self.http.get(&url).header("Accept", "application/vnd.github+json").header("X-GitHub-Api-Version", "2022-11-28");
        if let Some(t) = self.github_token() {
            rq = rq.bearer_auth(t);
        }
        let resp = rq.send().await.map_err(|e| anyhow!("GitHub request failed: {e}"))?;
        match resp.status().as_u16() {
            200 => {}
            404 => bail!("GitHub has no repository '{repo}' (private repositories need a token in Settings)"),
            403 | 429 => bail!("GitHub API refused the request; you are probably rate limited, add a GitHub token in Settings"),
            other => bail!("GitHub API returned {other} for {repo}"),
        }
        let v: Value = resp.json().await.map_err(|e| anyhow!("bad GitHub response: {e}"))?;
        let s = |k: &str| v[k].as_str().unwrap_or("").to_string();
        Ok(RepoInfo { full_name: s("full_name"), name: s("name"), description: s("description") })
    }

    /// Whatever is cached right now, without touching the network.
    pub async fn cached_releases(&self, id: &str) -> Vec<Release> {
        self.releases.lock().await.get(id).map(|c| c.list.clone()).unwrap_or_default()
    }

    // ---- the release feed ---------------------------------------------------------------

    /// Fetches ArtCraft's release feed (at most every two minutes) and files each suite app's
    /// latest release as a hint, so a routine check needs no GitHub request. Failure is harmless:
    /// GitHub answers instead.
    pub async fn refresh_feed(&self, force: bool) {
        let Some(url) = self.cfg.feed_url.clone() else { return };
        if !force {
            if let Some((at, _)) = self.feed.lock().unwrap().as_ref() {
                if at.elapsed() < FEED_TTL {
                    return;
                }
            }
        }
        let text = match self.http.get(&url).send().await.and_then(|r| r.error_for_status()) {
            Ok(r) => match r.text().await {
                Ok(t) if t.len() < 8 * 1024 * 1024 => t,
                _ => return,
            },
            Err(e) => {
                tracing::debug!("release feed unavailable: {e}");
                return;
            }
        };
        let Some(feed) = Feed::parse(&text) else { return };
        let t = now() as i64;
        let mut hints = BTreeMap::new();
        for def in self.catalog_snapshot() {
            let Some(app) = CraftApp::by_id(&def.id) else { continue };
            if !def.repo.eq_ignore_ascii_case(app.repo) {
                continue;
            }
            if let FeedAnswer::Fresh(Some(r)) = feed.answer(app, t) {
                let rel = build_release(r, &def);
                if rel.web.is_some() {
                    hints.insert(def.id.clone(), rel);
                }
            }
        }
        *self.feed_hints.lock().unwrap() = hints;
        *self.feed.lock().unwrap() = Some((Instant::now(), feed));
    }

    pub fn feed_hint(&self, id: &str) -> Option<Release> {
        self.feed_hints.lock().unwrap().get(id).cloned()
    }

    /// The release to offer as latest: the feed's hint or the cached GitHub list, whichever is newer.
    pub async fn latest(&self, id: &str, include_prerelease: bool) -> Option<Release> {
        let list = self.cached_releases(id).await;
        let from_list = pick_latest(&list, include_prerelease).cloned();
        let hint = self.feed_hint(id).filter(|h| include_prerelease || !h.prerelease);
        match (from_list, hint) {
            (Some(a), Some(b)) => match (&a.semver, &b.semver) {
                (Some(x), Some(y)) if version::is_newer(y, x) => Some(b),
                _ => Some(a),
            },
            (a, b) => a.or(b),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use release::Asset;

    fn def() -> AppDef {
        AppDef { id: "photocraft".into(), name: "PhotoCraft".into(), repo: "storytold/photocraft".into(), description: String::new(), web_asset_contains: "-web-".into(), icon: None, disabled: false, custom: false }
    }

    fn core_release() -> release::Release {
        let names = [
            "photocraft-web-0.1.1-rc.4.zip",
            "photocraft-0.1.1-rc.4-linux-x86_64.AppImage",
            "photocraft-0.1.1-rc.4-windows-x64-portable.zip",
            "photocraft-0.1.1-rc.4-windows-x64.msi",
            "photocraft-0.1.1-rc.4-macos-universal.dmg",
            "photocraft-cli-0.1.1-rc.4-macos-universal.zip",
            "SHA256SUMS.txt",
        ];
        release::Release {
            tag: "v0.1.1-rc.4".into(),
            version: version::parse("v0.1.1-rc.4"),
            prerelease: true,
            assets: names.iter().map(|n| Asset { name: (*n).into(), size: 1, url: format!("https://github.com/storytold/photocraft/releases/download/v0.1.1-rc.4/{n}"), sha256: None }).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn splits_web_and_desktop_assets() {
        let rel = build_release(core_release(), &def());
        assert_eq!(rel.version, "0.1.1-rc.4");
        assert_eq!(rel.web.as_ref().unwrap().name, "photocraft-web-0.1.1-rc.4.zip");
        assert!(rel.sums_url.is_some());
        assert_eq!(rel.native.len(), 5);
        assert!(rel.native.iter().any(|n| n.os == "macos" && n.kind == "dmg"));
        assert!(rel.native.iter().any(|n| n.kind == "cli"));
    }

    /// The SolidCraft 0.14.0 release page: versioned and unversioned copies of every file, `.snap`, `.exe`,
    /// `.zsync` files and the odd leftover `solvecraft-*` zsync files next to the one web zip.
    #[test]
    fn solidcraft_0_14_0_release_page() {
        let names = [
            "SHA256SUMS.txt",
            "solidcraft-0.14.0-freebsd-x86_64.tar.gz",
            "solidcraft-0.14.0-linux-aarch64.AppImage",
            "solidcraft-0.14.0-linux-aarch64.AppImage.zsync",
            "solidcraft-0.14.0-linux-aarch64.deb",
            "solidcraft-0.14.0-linux-aarch64.snap",
            "solidcraft-0.14.0-linux-x86_64.AppImage",
            "solidcraft-0.14.0-linux-x86_64.deb",
            "solidcraft-0.14.0-linux-x86_64.snap",
            "solidcraft-0.14.0-macos-universal.dmg",
            "solidcraft-0.14.0-windows-x64-portable.zip",
            "SolidCraft-0.14.0-windows-x64.exe",
            "solidcraft-0.14.0-windows-x64.msi",
            "solidcraft-0.14.0-windows-arm64.msi",
            "solidcraft-cli-0.14.0-macos-universal.zip",
            "solidcraft-linux-x86_64.AppImage",
            "solidcraft-macos-universal.dmg",
            "solidcraft-windows-x64.msi",
            "solidcraft-web-0.14.0.zip",
            "solvecraft-0.14.0-linux-x86_64.AppImage.zsync",
        ];
        let web_sha = "643095ef35d0b2227d504be06d69c42cb1858197762cd02fff3ccfdf73c4b84b";
        let core = release::Release {
            tag: "v0.14.0".into(),
            version: version::parse("v0.14.0"),
            assets: names
                .iter()
                .map(|n| Asset {
                    name: (*n).into(),
                    size: 1,
                    url: format!("https://github.com/bherbruck/solidcraft/releases/download/v0.14.0/{n}"),
                    sha256: (*n == "solidcraft-web-0.14.0.zip").then(|| web_sha.to_string()),
                })
                .collect(),
            ..Default::default()
        };
        let def = AppDef { id: "solidcraft".into(), name: "SolidCraft".into(), repo: "bherbruck/solidcraft".into(), description: String::new(), web_asset_contains: "-web-".into(), icon: None, disabled: false, custom: false };
        let stale = AppDef { web_asset_contains: "solvecraft-web".into(), ..def.clone() };
        assert_eq!(build_release(core.clone(), &stale).web.as_ref().map(|w| w.name.as_str()), Some("solidcraft-web-0.14.0.zip"));
        let rel = build_release(core, &def);
        let web = rel.web.as_ref().expect("web build found");
        assert_eq!(web.name, "solidcraft-web-0.14.0.zip");
        assert_eq!(web.sha256.as_deref(), Some(web_sha));
        assert!(rel.sums_url.is_some());
        for n in &rel.native {
            assert!(!n.name.ends_with(".exe") && !n.name.ends_with(".snap") && !n.name.contains("zsync"), "{}", n.name);
            assert!(n.name.contains("0.14.0"), "unversioned copy listed: {}", n.name);
        }
        let d = downloads_for(&rel, &["solidcraft"]);
        assert_eq!(d["windows-x64"].kind, "msi");
        assert_eq!(d["linux-x64"].name, "solidcraft-0.14.0-linux-x86_64.AppImage");
        assert_eq!(d["macos"].kind, "dmg");
    }

    #[test]
    fn picks_downloads_per_platform() {
        let rel = build_release(core_release(), &def());
        let d = downloads_for(&rel, &["photocraft"]);
        assert_eq!(d["windows-x64"].kind, "msi");
        assert_eq!(d["windows-arm64"].name, "photocraft-0.1.1-rc.4-windows-x64.msi");
        assert_eq!(d["linux-x64"].kind, "appimage");
        assert_eq!(d["macos"].kind, "dmg");
        assert!(!d.contains_key("linux-arm64"));
    }

    #[test]
    fn latest_is_by_version_not_list_order() {
        let mk = |tag: &str, pre: bool| {
            let mut c = core_release();
            c.tag = tag.into();
            c.version = version::parse(tag);
            c.prerelease = pre;
            build_release(c, &def())
        };
        let list = vec![mk("v0.1.0", false), mk("v0.2.0-rc.1", true), mk("v0.1.5", false)];
        assert_eq!(pick_latest(&list, false).unwrap().tag, "v0.1.5");
        assert_eq!(pick_latest(&list, true).unwrap().tag, "v0.2.0-rc.1");
    }

    #[test]
    fn follows_a_same_owner_repo_rename_only() {
        let item = serde_json::json!({"html_url": "https://github.com/bherbruck/solidcraft/releases/tag/v1"});
        assert_eq!(canonical_repo(&item, "bherbruck/solvecraft"), "bherbruck/solidcraft");
        let other = serde_json::json!({"html_url": "https://github.com/evil/solvecraft/releases/tag/v1"});
        assert_eq!(canonical_repo(&other, "bherbruck/solvecraft"), "bherbruck/solvecraft");
    }
}
