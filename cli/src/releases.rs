use anyhow::{anyhow, bail, Context, Result};
use flate2::read::GzDecoder;
use indicatif::{ProgressBar, ProgressStyle};
use serde::Deserialize;
use std::fs;
use std::io::{Cursor, Read};
use std::path::Path;
use tar::Archive;

pub const DEFAULT_REPO: &str = "getomnico/omni";
pub const DOCKER_COMPOSE_ASSET: &str = "omni-docker-compose.tar.gz";

#[derive(Debug, Clone, Deserialize)]
pub struct GitHubRelease {
    pub tag_name: String,
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub prerelease: bool,
    #[serde(default)]
    pub assets: Vec<GitHubAsset>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GitHubAsset {
    pub name: String,
    pub browser_download_url: String,
    #[serde(default)]
    pub size: u64,
}

pub fn normalize_tag(version: &str) -> String {
    if version.starts_with('v') {
        version.to_string()
    } else {
        format!("v{version}")
    }
}

pub async fn resolve_release(version: Option<&str>) -> Result<GitHubRelease> {
    let client = reqwest::Client::builder()
        .user_agent(format!("omni-cli/{}", env!("CARGO_PKG_VERSION")))
        .build()?;

    match version {
        Some(version) => release_by_tag(&client, DEFAULT_REPO, &normalize_tag(version)).await,
        None => latest_stable_release(&client, DEFAULT_REPO).await,
    }
}

pub async fn latest_stable_release(client: &reqwest::Client, repo: &str) -> Result<GitHubRelease> {
    let url = format!("https://api.github.com/repos/{repo}/releases");
    let releases: Vec<GitHubRelease> = client
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    releases
        .into_iter()
        .find(|release| !release.draft && !release.prerelease)
        .ok_or_else(|| anyhow!("no stable GitHub releases found for {repo}"))
}

pub async fn release_by_tag(
    client: &reqwest::Client,
    repo: &str,
    tag: &str,
) -> Result<GitHubRelease> {
    let url = format!("https://api.github.com/repos/{repo}/releases/tags/{tag}");
    let release: GitHubRelease = client
        .get(url)
        .send()
        .await?
        .error_for_status()
        .with_context(|| format!("GitHub release {tag} not found"))?
        .json()
        .await?;
    Ok(release)
}

impl GitHubRelease {
    pub fn asset(&self, name: &str) -> Result<&GitHubAsset> {
        self.assets
            .iter()
            .find(|asset| asset.name == name)
            .ok_or_else(|| anyhow!("release {} does not contain asset {name}", self.tag_name))
    }
}

pub async fn download_asset(asset: &GitHubAsset, destination: &Path) -> Result<()> {
    let client = reqwest::Client::builder()
        .user_agent(format!("omni-cli/{}", env!("CARGO_PKG_VERSION")))
        .build()?;

    let response = client
        .get(&asset.browser_download_url)
        .send()
        .await?
        .error_for_status()?;
    let bytes = response.bytes().await?;

    let total = if asset.size > 0 {
        asset.size
    } else {
        bytes.len() as u64
    };
    let pb = ProgressBar::new(total);
    pb.set_style(
        ProgressStyle::with_template("{spinner:.green} downloaded {bytes}/{total_bytes} {msg}")
            .unwrap_or_else(|_| ProgressStyle::default_spinner()),
    );
    fs::write(destination, &bytes)
        .with_context(|| format!("failed to write {}", destination.display()))?;
    pb.inc(bytes.len() as u64);
    pb.finish_with_message(asset.name.clone());
    Ok(())
}

pub fn extract_docker_compose_archive(archive_path: &Path, destination: &Path) -> Result<()> {
    fs::create_dir_all(destination)
        .with_context(|| format!("failed to create {}", destination.display()))?;
    let mut bytes = Vec::new();
    fs::File::open(archive_path)
        .with_context(|| format!("failed to open {}", archive_path.display()))?
        .read_to_end(&mut bytes)?;
    extract_tgz_bytes(&bytes, destination)
}

pub fn extract_tgz_bytes(bytes: &[u8], destination: &Path) -> Result<()> {
    let decoder = GzDecoder::new(Cursor::new(bytes));
    let mut archive = Archive::new(decoder);
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.to_path_buf();
        if path.is_absolute()
            || path
                .components()
                .any(|component| matches!(component, std::path::Component::ParentDir))
        {
            bail!("archive contains unsafe path {}", path.display());
        }
        entry.unpack_in(destination)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_release_tags() {
        assert_eq!(normalize_tag("1.2.3"), "v1.2.3");
        assert_eq!(normalize_tag("v1.2.3"), "v1.2.3");
    }

    #[test]
    fn finds_named_asset() {
        let release = GitHubRelease {
            tag_name: "v1.0.0".into(),
            draft: false,
            prerelease: false,
            assets: vec![GitHubAsset {
                name: DOCKER_COMPOSE_ASSET.into(),
                browser_download_url: "https://example.com/a.tgz".into(),
                size: 1,
            }],
        };
        assert_eq!(release.asset(DOCKER_COMPOSE_ASSET).unwrap().size, 1);
    }
}
