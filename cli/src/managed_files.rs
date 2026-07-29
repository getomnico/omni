use anyhow::{Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use similar::TextDiff;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

pub const MANAGED_FILES: &[&str] = &[
    "docker/docker-compose.yml",
    "docker/docker-compose.local-inference.yml",
    "Caddyfile",
    ".env",
];

pub const MANIFEST_PATH: &str = ".omni/managed-files.json";

#[derive(Debug, Clone, Serialize)]
pub struct FileChange {
    pub path: String,
    pub source_path: String,
    pub exists_locally: bool,
    pub changed: bool,
    pub local_edit_detected: bool,
    pub diff: Option<String>,
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

fn source_file(release_root: &Path, relative: &str) -> PathBuf {
    // For .env, the release ships it as .env.example
    if relative == ".env" {
        release_root.join(".env.example")
    } else {
        release_root.join(relative)
    }
}

fn analyze_one(
    root: &Path,
    release_root: &Path,
    relative: &str,
    manifest: &ManagedManifest,
) -> Result<FileChange> {
    let local = root.join(relative);
    let incoming = source_file(release_root, relative);
    let source_path = if relative == ".env" {
        ".env.example".to_string()
    } else {
        relative.to_string()
    };
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

    let diff = if exists_locally && incoming.exists() {
        compute_file_diff(&local, &incoming)
    } else {
        None
    };

    Ok(FileChange {
        path: relative.to_string(),
        source_path,
        exists_locally,
        changed,
        local_edit_detected,
        diff,
    })
}

fn compute_file_diff(local: &Path, incoming: &Path) -> Option<String> {
    let incoming_text = fs::read_to_string(incoming).ok()?;
    let local_text = fs::read_to_string(local).ok()?;

    if local_text == incoming_text {
        return None;
    }

    let diff = TextDiff::from_lines(&incoming_text, &local_text);
    let fname = incoming
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("file");

    Some(format!(
        "{}",
        diff.unified_diff()
            .context_radius(3)
            .header(&format!("a/{fname}"), &format!("b/{fname}"))
    ))
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
        // .env is handled separately by build_env_plan; still update .env.example reference
        if *relative == ".env" {
            let source = release_root.join(".env.example");
            if source.exists() {
                let destination = root.join(".env.example");
                if let Some(parent) = destination.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::copy(&source, &destination).with_context(|| {
                    format!("failed to replace .env.example from release asset")
                })?;
            }
            continue;
        }

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
    fn diff_is_collapsible_hunk_based() {
        // Simulates a file with scattered changes — long unchanged regions should collapse
        let tmp = tempfile::tempdir().unwrap();
        let release = tmp.path().join("release");
        let local = tmp.path().join("local");
        fs::create_dir_all(release.join("docker")).unwrap();
        fs::create_dir_all(local.join("docker")).unwrap();

        let old = r#"services:
  postgres:
    image: postgres:16
    ports:
      - "5432:5432"

  searcher:
    image: omni/searcher:latest
    environment:
      - FOO=bar

  redis:
    image: redis:7
"#;
        let new = r#"services:
  postgres:
    image: postgres:17
    ports:
      - "5432:5432"

  search-service:
    image: omni/search:latest
    environment:
      - FOO=bar
      - BAZ=qux

  redis:
    image: redis:7-alpine
"#;

        fs::write(release.join("docker/docker-compose.yml"), old).unwrap();
        fs::write(local.join("docker/docker-compose.yml"), new).unwrap();

        let changes = analyze(&local, &release).unwrap();
        let change = changes
            .iter()
            .find(|c| c.path == "docker/docker-compose.yml")
            .unwrap();

        assert!(change.changed);
        let diff = change.diff.as_deref().unwrap();

        // Should have @@ hunk headers (collapsible style), not every line
        assert!(diff.contains("@@"), "diff should contain hunk headers");
        // Should NOT contain every single line from the file
        let context_line_count = diff.lines().filter(|l| l.starts_with(' ')).count();
        assert!(
            context_line_count <= 10,
            "context lines should be limited, got {context_line_count}"
        );
        // Should show the changed lines
        assert!(diff.contains("+    image: postgres:17"));
        assert!(diff.contains("-    image: postgres:16"));
        assert!(diff.contains("+  search-service:"));
        assert!(diff.contains("-  searcher:"));
        assert!(diff.contains("+      - BAZ=qux"));
        assert!(diff.contains("+    image: redis:7-alpine"));
        assert!(diff.contains("-    image: redis:7"));

        eprintln!("\n=== Validated collapsible diff output ===\n{diff}\n");
    }

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
