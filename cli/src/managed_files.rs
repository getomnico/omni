use anyhow::{Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

pub const MANAGED_FILES: &[&str] = &[
    "docker/docker-compose.yml",
    "docker/docker-compose.local-inference.yml",
    "Caddyfile",
    ".env.example",
];

pub const MANIFEST_PATH: &str = ".omni/managed-files.json";

#[derive(Debug, Clone, Serialize)]
pub struct FileChange {
    pub path: String,
    pub exists_locally: bool,
    pub changed: bool,
    pub local_edit_detected: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct ManagedManifest {
    files: BTreeMap<String, String>,
}

pub fn analyze(root: &Path, release_root: &Path) -> Result<Vec<FileChange>> {
    let manifest = load_manifest(root).unwrap_or_default();
    MANAGED_FILES
        .iter()
        .map(|relative| analyze_one(root, release_root, relative, &manifest))
        .collect()
}

pub fn manifest_exists(root: &Path) -> bool {
    root.join(MANIFEST_PATH).exists()
}

pub fn mark_local_edits_against_base(
    changes: &mut [FileChange],
    root: &Path,
    base_root: &Path,
) -> Result<()> {
    for change in changes {
        let local = root.join(&change.path);
        let base = base_root.join(&change.path);
        change.local_edit_detected = if local.exists() && base.exists() {
            fs::read(&local)? != fs::read(&base)?
        } else {
            local.exists() != base.exists()
        };
    }
    Ok(())
}

pub fn mark_changed_existing_files_as_local_edits(changes: &mut [FileChange]) {
    for change in changes {
        if change.exists_locally && change.changed {
            change.local_edit_detected = true;
        }
    }
}

fn analyze_one(
    root: &Path,
    release_root: &Path,
    relative: &str,
    manifest: &ManagedManifest,
) -> Result<FileChange> {
    let local = root.join(relative);
    let incoming = release_root.join(relative);
    let exists_locally = local.exists();
    let changed = if exists_locally && incoming.exists() {
        fs::read(&local)? != fs::read(&incoming)?
    } else {
        incoming.exists() || exists_locally
    };

    let local_edit_detected = if let Some(recorded_hash) = manifest.files.get(relative) {
        exists_locally && hash_file(&local)? != *recorded_hash
    } else {
        false
    };

    Ok(FileChange {
        path: relative.to_string(),
        exists_locally,
        changed,
        local_edit_detected,
    })
}

pub fn create_backup(root: &Path, extra_paths: &[&str]) -> Result<PathBuf> {
    let timestamp = Utc::now().format("%Y%m%d-%H%M%S").to_string();
    let backup_dir = root.join(".omni/backups").join(timestamp);
    fs::create_dir_all(&backup_dir)
        .with_context(|| format!("failed to create backup directory {}", backup_dir.display()))?;

    let mut paths = MANAGED_FILES.to_vec();
    for path in extra_paths {
        if !paths.contains(path) {
            paths.push(path);
        }
    }

    for relative in paths {
        let source = root.join(relative);
        if source.exists() {
            let destination = backup_dir.join(relative);
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(&source, &destination).with_context(|| {
                format!(
                    "failed to back up {} to {}",
                    source.display(),
                    destination.display()
                )
            })?;
        }
    }

    Ok(backup_dir)
}

pub fn replace_managed_files(
    root: &Path,
    release_root: &Path,
    dry_run: bool,
) -> Result<Vec<FileChange>> {
    let changes = analyze(root, release_root)?;
    if dry_run {
        return Ok(changes);
    }

    for relative in MANAGED_FILES {
        let source = release_root.join(relative);
        if !source.exists() {
            continue;
        }
        let destination = root.join(relative);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(&source, &destination).with_context(|| {
            format!(
                "failed to replace managed file {} from release asset",
                destination.display()
            )
        })?;
    }
    save_manifest(root)?;
    Ok(changes)
}

fn load_manifest(root: &Path) -> Result<ManagedManifest> {
    let path = root.join(MANIFEST_PATH);
    let content = fs::read_to_string(&path)?;
    Ok(serde_json::from_str(&content)?)
}

fn save_manifest(root: &Path) -> Result<()> {
    let mut manifest = ManagedManifest::default();
    for relative in MANAGED_FILES {
        let path = root.join(relative);
        if path.exists() {
            manifest
                .files
                .insert(relative.to_string(), hash_file(&path)?);
        }
    }
    let path = root.join(MANIFEST_PATH);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, serde_json::to_string_pretty(&manifest)?)?;
    Ok(())
}

fn hash_file(path: &Path) -> Result<String> {
    Ok(format!("{:016x}", fnv1a64(&fs::read(path)?)))
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn backs_up_managed_files_and_env() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("docker")).unwrap();
        fs::write(tmp.path().join("docker/docker-compose.yml"), "a").unwrap();
        fs::write(tmp.path().join(".env"), "OMNI_VERSION=old\n").unwrap();
        let backup = create_backup(tmp.path(), &[".env"]).unwrap();
        assert!(backup.join("docker/docker-compose.yml").exists());
        assert!(backup.join(".env").exists());
    }

    #[test]
    fn replaces_managed_files_and_records_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        let release = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("docker")).unwrap();
        fs::create_dir_all(release.path().join("docker")).unwrap();
        fs::write(tmp.path().join("docker/docker-compose.yml"), "old").unwrap();
        fs::write(release.path().join("docker/docker-compose.yml"), "new").unwrap();
        replace_managed_files(tmp.path(), release.path(), false).unwrap();
        assert_eq!(
            fs::read_to_string(tmp.path().join("docker/docker-compose.yml")).unwrap(),
            "new"
        );
        assert!(manifest_exists(tmp.path()));
    }

    #[test]
    fn detects_first_upgrade_local_edits_against_base_release() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        let base = tempfile::tempdir().unwrap();
        for dir in [tmp.path(), target.path(), base.path()] {
            fs::create_dir_all(dir.join("docker")).unwrap();
        }
        fs::write(base.path().join("docker/docker-compose.yml"), "old").unwrap();
        fs::write(
            tmp.path().join("docker/docker-compose.yml"),
            "locally edited",
        )
        .unwrap();
        fs::write(target.path().join("docker/docker-compose.yml"), "new").unwrap();

        let mut changes = analyze(tmp.path(), target.path()).unwrap();
        assert!(!changes[0].local_edit_detected);
        mark_local_edits_against_base(&mut changes, tmp.path(), base.path()).unwrap();
        assert!(changes[0].local_edit_detected);
    }
}
