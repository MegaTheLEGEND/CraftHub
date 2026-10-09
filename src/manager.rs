//! Installing, activating, pruning and auto-updating web builds.

use crate::config::is_safe_name;
use crate::github::pick_latest;
use crate::state::{now, AppState, Installed};
use anyhow::{anyhow, bail, Context, Result};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    time::Duration,
};
use zip::ZipArchive;
use tokio::io::AsyncWriteExt;

const MAX_UNPACKED_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const COMPRESSIBLE: &[&str] = &["wasm", "js", "mjs", "html", "css", "json", "svg", "txt", "map"];

fn version_dir(st: &AppState, app: &str, version: &str) -> PathBuf {
    st.apps_dir().join(app).join(version)
}

/// Download, verify, unpack and register one release. Updates the job entry as it goes.
pub async fn install(st: AppState, app: String, tag: String, activate: bool) -> Result<()> {
    st.set_job(&app, |j| {
        j.state = "downloading".into();
        j.tag = tag.clone();
        j.message = "Looking up release".into();
        j.done = 0;
        j.total = 0;
    });
    let result = install_inner(&st, &app, &tag, activate).await;
    match &result {
        Ok(()) => st.set_job(&app, |j| {
            j.state = "done".into();
            j.message = format!("Installed {tag}");
        }),
        Err(e) => {
            tracing::warn!(app = %app, %tag, "install failed: {e:#}");
            let msg = format!("{e:#}");
            st.set_job(&app, |j| {
                j.state = "error".into();
                j.message = msg.clone();
            });
            let _ = st.store.update(&app, |r| r.last_error = Some(msg)).await;
        }
    }
    result
}

async fn install_inner(st: &AppState, app: &str, tag: &str, activate: bool) -> Result<()> {
    let mut releases = st.releases(app, false).await?;
    if !releases.iter().any(|r| r.tag == tag) {
        releases = st.releases(app, true).await?;
    }
    let rel = releases
        .iter()
        .find(|r| r.tag == tag)
        .cloned()
        .ok_or_else(|| anyhow!("release '{tag}' not found for {app}"))?;
    let web = rel.web.clone().ok_or_else(|| anyhow!("release {tag} has no web build"))?;
    let version = rel.version.clone();
    if !is_safe_name(&version) {
        bail!("refusing unsafe version string '{version}'");
    }

    let tmp_dir = st.cfg.data_dir.join("tmp");
    tokio::fs::create_dir_all(&tmp_dir).await?;
    let zip_path = tmp_dir.join(format!("{app}-{version}.zip"));

    // ---- download ---------------------------------------------------------
    let mut resp = st.http.get(&web.url).send().await?.error_for_status()?;
    let total = resp.content_length().unwrap_or(web.size);
    st.set_job(app, |j| {
        j.total = total;
        j.message = format!("Downloading {}", web.name);
    });
    let mut file = tokio::fs::File::create(&zip_path).await?;
    let mut hasher = Sha256::new();
    let mut done = 0u64;
    let mut last_report = 0u64;
    while let Some(chunk) = resp.chunk().await? {
        hasher.update(&chunk);
        file.write_all(&chunk).await?;
        done += chunk.len() as u64;
        if done - last_report > 256 * 1024 {
            last_report = done;
            st.set_job(app, |j| j.done = done);
        }
    }
    file.flush().await?;
    drop(file);
    st.set_job(app, |j| j.done = done);
    let got = hex::encode(hasher.finalize());

    // ---- verify -----------------------------------------------------------
    let expected = match &web.sha256 {
        Some(s) => Some(s.clone()),
        None => match &rel.sums_url {
            Some(url) => fetch_sum(st, url, &web.name).await,
            None => None,
        },
    };
    let verified = match expected {
        Some(exp) if exp.eq_ignore_ascii_case(&got) => true,
        Some(exp) => {
            let _ = tokio::fs::remove_file(&zip_path).await;
            bail!("checksum mismatch for {} (expected {exp}, got {got})", web.name);
        }
        None => {
            tracing::warn!(app, tag, "no published checksum found; installing unverified");
            false
        }
    };

    // ---- unpack -----------------------------------------------------------
    st.set_job(app, |j| {
        j.state = "extracting".into();
        j.message = "Unpacking and precompressing".into();
    });
    let app_root = st.apps_dir().join(app);
    tokio::fs::create_dir_all(&app_root).await?;
    let partial = app_root.join(format!("{version}.partial"));
    let final_dir = version_dir(st, app, &version);
    let (zp, pd, fd) = (zip_path.clone(), partial, final_dir);
    let size = tokio::task::spawn_blocking(move || unpack(&zp, &pd, &fd)).await??;
    let _ = tokio::fs::remove_file(&zip_path).await;

    // ---- register ---------------------------------------------------------
    let entry = Installed {
        version: version.clone(),
        tag: rel.tag.clone(),
        installed_at: now(),
        size,
        verified,
        prerelease: rel.prerelease,
    };
    st.store
        .update(app, |r| {
            r.installed.retain(|i| i.version != entry.version);
            r.installed.push(entry);
            if activate || r.active.is_none() {
                r.active = Some(version.clone());
            }
            r.last_error = None;
        })
        .await?;
    prune(st, app).await?;
    tracing::info!(app, version = %version, verified, "installed");
    Ok(())
}

async fn fetch_sum(st: &AppState, url: &str, file_name: &str) -> Option<String> {
    let text = st.http.get(url).send().await.ok()?.error_for_status().ok()?.text().await.ok()?;
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let (Some(hash), Some(name)) = (parts.next(), parts.next()) else { continue };
        if name.trim_start_matches('*') == file_name && hash.len() == 64 {
            return Some(hash.to_ascii_lowercase());
        }
    }
    None
}

/// Extract `zip_path` into `partial` (stripping a single top-level folder), precompress,
/// then atomically move it into place. Returns the unpacked size in bytes.
fn unpack(zip_path: &Path, partial: &Path, final_dir: &Path) -> Result<u64> {
    if partial.exists() {
        fs::remove_dir_all(partial)?;
    }
    fs::create_dir_all(partial)?;
    let result = unpack_into(zip_path, partial);
    if let Err(e) = result {
        let _ = fs::remove_dir_all(partial);
        return Err(e);
    }
    let size = result?;
    if !partial.join("index.html").is_file() {
        let _ = fs::remove_dir_all(partial);
        bail!("archive has no index.html at its root; is this really a web build?");
    }
    if let Err(e) = precompress(partial) {
        tracing::warn!("precompression skipped: {e:#}");
    }
    if final_dir.exists() {
        fs::remove_dir_all(final_dir)?;
    }
    fs::rename(partial, final_dir)?;
    Ok(size)
}

fn unpack_into(zip_path: &Path, dest: &Path) -> Result<u64> {
    let file = fs::File::open(zip_path)?;
    let mut archive = zip::ZipArchive::new(file).context("not a valid zip archive")?;

    // Work out whether everything lives under one top-level folder.
    let mut first: Option<std::ffi::OsString> = None;
    let mut strip = true;
    for i in 0..archive.len() {
        let entry = archive.by_index(i)?;
        let Some(p) = entry.enclosed_name() else { continue };
        let mut comps = p.components();
        let Some(Component::Normal(head)) = comps.next() else { continue };
        let has_rest = comps.next().is_some();
        if !has_rest && !entry.is_dir() {
            strip = false;
            break;
        }
        match &first {
            None => first = Some(head.to_os_string()),
            Some(f) if f == head => {}
            Some(_) => {
                strip = false;
                break;
            }
        }
    }

    let mut total = 0u64;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        let Some(rel) = entry.enclosed_name() else { continue };
        let rel: PathBuf = if strip { rel.components().skip(1).collect() } else { rel };
        if rel.as_os_str().is_empty() {
            continue;
        }
        if let Some(mode) = entry.unix_mode() {
            if mode & 0o170000 == 0o120000 {
                continue; // never materialise symlinks
            }
        }
        let out = dest.join(&rel);
        if entry.is_dir() {
            fs::create_dir_all(&out)?;
            continue;
        }
        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent)?;
        }
        let remaining = MAX_UNPACKED_BYTES.saturating_sub(total);
        let mut f = fs::File::create(&out)?;
        let written = std::io::copy(&mut (&mut entry).take(remaining + 1), &mut f)?;
        total += written;
        if total > MAX_UNPACKED_BYTES {
            bail!("archive expands beyond {} bytes; refusing", MAX_UNPACKED_BYTES);
        }
    }
    Ok(total)
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let ft = entry.file_type()?;
        if ft.is_dir() {
            walk(&path, out)?;
        } else if ft.is_file() {
            out.push(path);
        }
    }
    Ok(())
}

/// Write `.br` and `.gz` siblings so the server can send pre-compressed bytes
/// (the wasm payload is ~13 MB raw, ~5 MB compressed).
fn precompress(root: &Path) -> Result<()> {
    let mut files = vec![];
    walk(root, &mut files)?;
    for path in files {
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        if !COMPRESSIBLE.contains(&ext.as_str()) {
            continue;
        }
        let data = fs::read(&path)?;
        if data.len() < 1024 {
            continue;
        }
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();

        let br_path = path.with_file_name(format!("{name}.br"));
        let mut w = brotli::CompressorWriter::new(fs::File::create(&br_path)?, 64 * 1024, 9, 22);
        w.write_all(&data)?;
        w.flush()?;
        drop(w.into_inner());

        let gz_path = path.with_file_name(format!("{name}.gz"));
        let mut gz = flate2::write::GzEncoder::new(fs::File::create(&gz_path)?, flate2::Compression::best());
        gz.write_all(&data)?;
        gz.finish()?;
    }
    Ok(())
}

pub async fn activate(st: &AppState, app: &str, version: &str) -> Result<()> {
    let rec = st.store.record(app).await;
    if !rec.installed.iter().any(|i| i.version == version) {
        bail!("version {version} is not installed");
    }
    let version = version.to_string();
    st.store.update(app, |r| r.active = Some(version)).await?;
    Ok(())
}

pub async fn remove(st: &AppState, app: &str, version: &str) -> Result<()> {
    if !is_safe_name(version) {
        bail!("invalid version");
    }
    let rec = st.store.record(app).await;
    if rec.active.as_deref() == Some(version) {
        bail!("cannot remove the active version; activate another one first");
    }
    if !rec.installed.iter().any(|i| i.version == version) {
        bail!("version {version} is not installed");
    }
    let dir = version_dir(st, app, version);
    if dir.exists() {
        tokio::fs::remove_dir_all(&dir).await?;
    }
    let v = version.to_string();
    st.store.update(app, |r| r.installed.retain(|i| i.version != v)).await?;
    Ok(())
}

/// Keep the newest `KEEP_VERSIONS` plus the active one.
async fn prune(st: &AppState, app: &str) -> Result<()> {
    let rec = st.store.record(app).await;
    let mut inactive: Vec<&Installed> =
        rec.installed.iter().filter(|i| Some(&i.version) != rec.active.as_ref()).collect();
    inactive.sort_by_key(|i| std::cmp::Reverse(i.installed_at));
    let keep_inactive = st.cfg.keep_versions.saturating_sub(1);
    let doomed: Vec<String> = inactive.iter().skip(keep_inactive).map(|i| i.version.clone()).collect();
    for v in doomed {
        remove(st, app, &v).await?;
        tracing::info!(app, version = %v, "pruned old version");
    }
    Ok(())
}

pub fn effective_prerelease(st: &AppState, per_app: Option<bool>) -> bool {
    per_app.unwrap_or(st.cfg.include_prerelease)
}

/// Refresh one app's release list and, if enabled, install whatever is newest.
pub async fn check_app(st: &AppState, app: &str) -> Result<()> {
    let rec = st.store.record(app).await;
    let outcome: Result<()> = async {
        let list = st.releases(app, true).await?;
        let pre = effective_prerelease(st, rec.include_prerelease);
        let Some(latest) = pick_latest(&list, pre).cloned() else { return Ok(()) };
        let already = rec.installed.iter().any(|i| i.version == latest.version);
        let wanted = rec.auto_update && (st.cfg.auto_install || !rec.installed.is_empty());
        if wanted && !already && !st.job_running(app) {
            tracing::info!(app, version = %latest.version, "auto-installing");
            install(st.clone(), app.to_string(), latest.tag.clone(), true).await?;
        } else if wanted && already && rec.active.as_deref() != Some(latest.version.as_str()) {
            // Installed earlier but someone rolled back; leave their choice alone.
        }
        Ok(())
    }
    .await;
    let err = outcome.as_ref().err().map(|e| format!("{e:#}"));
    let _ = st
        .store
        .update(app, |r| {
            r.last_check = Some(now());
            if let Some(e) = err {
                r.last_error = Some(e);
            } else if r.last_error.is_some() {
                r.last_error = None;
            }
        })
        .await;
    outcome
}

pub async fn check_all(st: &AppState) {
    for def in st.catalog.clone() {
        if let Err(e) = check_app(st, &def.id).await {
            tracing::warn!(app = %def.id, "update check failed: {e:#}");
        }
    }
}

/// Background loop: first check shortly after boot, then every UPDATE_INTERVAL_MINUTES (default 30).
/// New releases are installed automatically unless AUTO_INSTALL is off or the app's own switch is off.
pub async fn updater(st: AppState) {
    let minutes = st.cfg.update_interval_minutes;
    if minutes == 0 {
        tracing::info!("automatic update checks disabled (UPDATE_INTERVAL_MINUTES=0)");
        return;
    }
    tokio::time::sleep(Duration::from_secs(3)).await;
    loop {
        check_all(&st).await;
        tokio::time::sleep(Duration::from_secs(minutes * 60)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    fn make_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let f = fs::File::create(path).unwrap();
        let mut z = zip::ZipWriter::new(f);
        let opts = zip::write::SimpleFileOptions::default();
        for (name, data) in entries {
            z.start_file(*name, opts).unwrap();
            z.write_all(data).unwrap();
        }
        z.finish().unwrap();
    }

    #[test]
    fn strips_single_top_level_folder_and_requires_index() {
        let dir = std::env::temp_dir().join(format!("craft-hub-t1-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let zip = dir.join("a.zip");
        make_zip(&zip, &[("site-1.0/index.html", b"<html></html>"), ("site-1.0/app.js", b"x")]);
        let size = unpack(&zip, &dir.join("v.partial"), &dir.join("v")).unwrap();
        assert_eq!(size, 14);
        assert!(dir.join("v/index.html").is_file());
        assert!(dir.join("v/app.js").is_file());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn rejects_archive_without_index() {
        let dir = std::env::temp_dir().join(format!("craft-hub-t2-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let zip = dir.join("a.zip");
        make_zip(&zip, &[("readme.txt", b"hello")]);
        assert!(unpack(&zip, &dir.join("v.partial"), &dir.join("v")).is_err());
        assert!(!dir.join("v").exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn flat_archive_is_not_stripped() {
        let dir = std::env::temp_dir().join(format!("craft-hub-t3-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let zip = dir.join("a.zip");
        make_zip(&zip, &[("index.html", b"<html></html>"), ("assets/x.js", b"x")]);
        unpack(&zip, &dir.join("v.partial"), &dir.join("v")).unwrap();
        assert!(dir.join("v/assets/x.js").is_file());
        fs::remove_dir_all(&dir).unwrap();
    }
}
