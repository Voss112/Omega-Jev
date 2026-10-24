//! Silent updates from the GitHub releases: newer tag → MSI downloaded, SHA-256 checked, installed on exit.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use reqwest::{Client, Url};
use serde::Deserialize;

use super::http::read_capped;

const LATEST: &str = "https://api.github.com/repos/Voss112/Omega-Jev/releases/latest";
const MAX_MSI: usize = 300 * 1024 * 1024;
const MAX_JSON: usize = 1024 * 1024;

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
    size: u64,
    /// `sha256:<hex>`, computed by GitHub at upload.
    digest: Option<String>,
}

/// Parses `v1.2.3` / `1.2.3` (pre-release suffix ignored).
fn version(tag: &str) -> Option<(u64, u64, u64)> {
    let core = tag.trim().trim_start_matches('v').split(['-', '+']).next()?;
    let mut parts = core.split('.').map(str::parse::<u64>);
    let v = (parts.next()?.ok()?, parts.next()?.ok()?, parts.next().unwrap_or(Ok(0)).ok()?);
    parts.next().is_none().then_some(v)
}

fn newer(tag: &str, current: &str) -> bool {
    matches!((version(tag), version(current)), (Some(t), Some(c)) if t > c)
}

/// Downloads only from GitHub over HTTPS (release assets redirect to githubusercontent).
fn trusted(url: &Url) -> bool {
    url.scheme() == "https"
        && url
            .host_str()
            .is_some_and(|h| h == "github.com" || h.ends_with(".github.com") || h.ends_with(".githubusercontent.com"))
}

fn sha256_hex(bytes: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, bytes).as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

/// Downloads the latest release MSI into `dir` when it is newer than `current`; `None` when up to date.
pub async fn fetch(http: &Client, current: &str, dir: &Path) -> Result<Option<PathBuf>> {
    let res = http.get(LATEST).header("Accept", "application/vnd.github+json").header("User-Agent", "Omega").send().await?;
    ensure!(res.status().is_success(), "GitHub releases : HTTP {}", res.status());
    let release: Release = serde_json::from_slice(&read_capped(res, MAX_JSON).await?)?;
    if release.draft || release.prerelease || !newer(&release.tag_name, current) {
        return Ok(None);
    }
    let asset = release
        .assets
        .iter()
        .find(|a| a.name.ends_with("-x64.msi") && a.name.starts_with("Omega-"))
        .context("release sans MSI")?;
    let expected = asset.digest.as_deref().and_then(|d| d.strip_prefix("sha256:")).context("MSI sans empreinte SHA-256")?;
    ensure!(asset.size as usize <= MAX_MSI, "MSI trop gros ({} o)", asset.size);
    let url = Url::parse(&asset.browser_download_url)?;
    ensure!(trusted(&url), "URL de téléchargement refusée : {url}");
    let res = http.get(url).header("User-Agent", "Omega").send().await?;
    ensure!(trusted(res.url()), "redirection refusée : {}", res.url());
    ensure!(res.status().is_success(), "téléchargement MSI : HTTP {}", res.status());
    let bytes = read_capped(res, MAX_MSI).await?;
    if !sha256_hex(&bytes).eq_ignore_ascii_case(expected) {
        bail!("empreinte SHA-256 du MSI invalide : mise à jour ignorée");
    }
    std::fs::create_dir_all(dir)?;
    // Only the newest installer is kept.
    for old in std::fs::read_dir(dir)?.flatten() {
        if old.file_name().to_string_lossy().starts_with("Omega-update-") {
            let _ = std::fs::remove_file(old.path());
        }
    }
    // Fixed local name: the remote asset name never reaches the file system or the command line.
    let path = dir.join(format!(
        "Omega-update-{}.msi",
        release.tag_name.trim_start_matches('v').replace(|c: char| !c.is_ascii_alphanumeric() && c != '.', "")
    ));
    std::fs::write(&path, &bytes)?;
    Ok(Some(path))
}

/// Starts the silent install; it outlives Omega, which must exit right after so files can be replaced.
pub fn install(msi: &Path) -> Result<()> {
    std::process::Command::new("msiexec")
        .args(["/i"])
        .arg(msi)
        .args(["/qn", "/norestart"])
        .spawn()
        .context("msiexec introuvable")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_strictly_newer_tags_update() {
        assert!(newer("v0.9.6", "0.9.5"));
        assert!(newer("v0.10.0", "0.9.5"));
        assert!(newer("1.0", "0.9.5"));
        assert!(!newer("v0.9.5", "0.9.5"));
        assert!(!newer("v0.9.4", "0.9.5"));
        assert!(!newer("latest", "0.9.5"));
        assert!(!newer("v1.2.3.4", "0.9.5"));
    }

    #[test]
    fn downloads_only_from_github_https() {
        let ok = |u: &str| trusted(&Url::parse(u).unwrap());
        assert!(ok("https://github.com/Voss112/Omega-Jev/releases/download/v1/Omega-1.0.0-x64.msi"));
        assert!(ok("https://objects.githubusercontent.com/x"));
        assert!(!ok("http://github.com/x"));
        assert!(!ok("https://github.com.evil.io/x"));
        assert!(!ok("https://evilgithub.com/x"));
    }

    #[test]
    fn digest_is_lowercase_sha256() {
        assert_eq!(sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }
}
