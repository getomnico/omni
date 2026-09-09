//! Git-based per-chat file versioning.
//!
//! Each chat directory is its own git repository. Every mutating request
//! (write, edit, bash, python) ends with an auto-commit so that artifacts can
//! be pinned to the exact version that was presented, and history can be
//! listed/restored later. Git failures are logged but never fail the request —
//! versioning is best-effort around the primary operation.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use tokio::process::Command;
use tokio::sync::{Mutex, Semaphore};
use tracing::debug;

const GIT_TIMEOUT: Duration = Duration::from_secs(15);
const GIT_AUTHOR_NAME: &str = "omni-sandbox";
const GIT_AUTHOR_EMAIL: &str = "sandbox@omni.local";
const GIT_IGNORE: &str = "_script.py\n";

/// Serializes git operations per chat directory. Concurrent requests for the
/// same chat would otherwise race on git's index.lock and lose commits.
#[derive(Default, Debug)]
pub struct GitLocks {
    locks: Mutex<HashMap<String, Arc<Semaphore>>>,
}

impl GitLocks {
    pub fn new() -> Self {
        Self::default()
    }

    async fn lock(&self, chat_id: &str) -> OwnedPermit {
        let sema = {
            let mut map = self.locks.lock().await;
            map.entry(chat_id.to_string())
                .or_insert_with(|| Arc::new(Semaphore::new(1)))
                .clone()
        };
        // Unwrap is safe: each semaphore has one permit and is never closed.
        OwnedPermit(sema.acquire_owned().await.unwrap())
    }
}

// Held across the critical section purely for its Drop guard effect.
struct OwnedPermit(#[allow(dead_code)] tokio::sync::OwnedSemaphorePermit);

fn git_command(chat_dir: &Path) -> Command {
    let mut cmd = Command::new("git");
    cmd.current_dir(chat_dir)
        .arg("-c")
        .arg(format!("user.name={GIT_AUTHOR_NAME}"))
        .arg("-c")
        .arg(format!("user.email={GIT_AUTHOR_EMAIL}"))
        .arg("-c")
        .arg("commit.gpgsign=false")
        .arg("-c")
        .arg("core.autocrlf=false")
        .env("HOME", chat_dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0");
    cmd
}

async fn run_git(chat_dir: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let output = tokio::time::timeout(GIT_TIMEOUT, async {
        git_command(chat_dir)
            .args(args)
            .output()
            .await
            .map_err(|e| format!("git spawn failed: {e}"))
    })
    .await
    .map_err(|_| "git command timed out".to_string())?
    .map_err(|e| format!("git failed: {e}"))?;

    if !output.status.success() {
        return Err(format!(
            "git {} failed: {}",
            args.first().unwrap_or(&""),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(output.stdout)
}

/// Ensure the chat directory is an initialized git repo with a .gitignore for
/// internal files. Does not create the directory.
pub async fn ensure_repo(chat_dir: &Path) {
    if !chat_dir.exists() || chat_dir.join(".git").exists() {
        return;
    }

    if let Err(e) = run_git(chat_dir, &["init", "-q"]).await {
        debug!(error = %e, "git init failed for chat dir");
        return;
    }
    let _ = tokio::fs::write(chat_dir.join(".gitignore"), GIT_IGNORE).await;
}

/// Commit all changes in the chat directory. No-op when nothing changed.
pub async fn commit(chat_dir: &Path, chat_id: &str, locks: &GitLocks, message: &str) {
    ensure_repo(chat_dir).await;
    let _guard = locks.lock(chat_id).await;

    if let Err(e) = run_git(chat_dir, &["add", "-A", "--", "."]).await {
        debug!(error = %e, "git add failed for chat dir");
        return;
    }

    // Exit code 1 with "nothing to commit" is expected and surfaces here as an
    // error string — treat it as success.
    if let Err(e) = run_git(chat_dir, &["commit", "-q", "-m", message, "--allow-empty"]).await
        && !e.contains("nothing to commit")
    {
        debug!(error = %e, "git commit failed for chat dir");
    }
}

/// SHA of the latest commit that touched `rel_path`, or None when the file has
/// no committed version yet.
pub async fn head_version(chat_dir: &Path, rel_path: &str) -> Option<String> {
    let output = run_git(
        chat_dir,
        &[
            "log",
            "-1",
            "--format=%H",
            "--",
            &sanitize_git_path(rel_path),
        ],
    )
    .await
    .ok()?;
    let sha = String::from_utf8_lossy(&output).trim().to_string();
    if sha.is_empty() { None } else { Some(sha) }
}

pub struct FileVersion {
    pub sha: String,
    pub timestamp: String,
    pub message: String,
}

/// Commit history for one file, newest first.
pub async fn list_versions(chat_dir: &Path, rel_path: &str) -> Result<Vec<FileVersion>, String> {
    let output = run_git(
        chat_dir,
        &[
            "log",
            "--format=%H%x1f%aI%x1f%s",
            "--",
            &sanitize_git_path(rel_path),
        ],
    )
    .await?;

    let text = String::from_utf8_lossy(&output);
    Ok(text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| {
            let mut parts = line.split('\x1f');
            Some(FileVersion {
                sha: parts.next()?.to_string(),
                timestamp: parts.next()?.to_string(),
                message: parts.next().unwrap_or("").to_string(),
            })
        })
        .collect())
}

/// File content at a specific commit. Empty history means git itself erred or
/// the repo is missing — callers turn that into a 404/502 upstream.
pub async fn read_version(chat_dir: &Path, sha: &str, rel_path: &str) -> Result<Vec<u8>, String> {
    if !sha.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err("Invalid version ref".into());
    }
    run_git(
        chat_dir,
        &["show", &format!("{sha}:{}", sanitize_git_path(rel_path))],
    )
    .await
}

/// Refuse paths git would interpret as options; callers already validated the
/// path stays inside the chat dir.
fn sanitize_git_path(rel_path: &str) -> String {
    if rel_path.starts_with('-') {
        format!("./{rel_path}")
    } else {
        rel_path.to_string()
    }
}

pub fn commit_message(tool: &str, path: Option<&str>) -> String {
    match path {
        Some(p) => format!("{tool}: {p}"),
        None => tool.to_string(),
    }
}
