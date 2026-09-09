use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::Json;
use axum::extract::{Query, State};
use axum::http::header;
use axum::response::Response;
use tokio::fs;

use crate::executor::{run_command, truncate_output};
use crate::models::*;
use crate::versioning;
use crate::{AppState, SandboxError};

fn get_chat_dir(scratch_dir: &Path, chat_id: &str) -> Result<PathBuf, SandboxError> {
    let safe_id = chat_id.replace('/', "").replace('\\', "").replace("..", "");
    if safe_id.is_empty() {
        return Err(SandboxError::BadRequest("Invalid chat_id".into()));
    }
    Ok(scratch_dir.join(safe_id))
}

fn validate_path(chat_dir: &Path, relative_path: &str) -> Result<PathBuf, SandboxError> {
    let full_path = chat_dir.join(relative_path);
    // Resolve the parent to check containment (the file itself may not exist yet)
    let parent = full_path
        .parent()
        .ok_or_else(|| SandboxError::BadRequest("Invalid path".into()))?;

    // For validation, we need the chat_dir to exist so we can canonicalize it
    let chat_dir_resolved = chat_dir
        .canonicalize()
        .map_err(|e| SandboxError::Internal(format!("Cannot resolve chat dir: {e}")))?;

    // If parent doesn't exist yet, walk up to find an existing ancestor
    let resolved = if parent.exists() {
        let parent_resolved = parent
            .canonicalize()
            .map_err(|e| SandboxError::Internal(format!("Cannot resolve path: {e}")))?;
        // Re-append the filename
        if let Some(name) = full_path.file_name() {
            parent_resolved.join(name)
        } else {
            parent_resolved
        }
    } else {
        // Parent doesn't exist — check that the relative path doesn't escape
        // by ensuring no ".." components after normalization
        let normalized: PathBuf = full_path
            .components()
            .filter(|c| !matches!(c, std::path::Component::ParentDir))
            .collect();
        if normalized != full_path {
            return Err(SandboxError::BadRequest(
                "Path traversal not allowed".into(),
            ));
        }
        full_path.clone()
    };

    if !resolved.starts_with(&chat_dir_resolved) {
        return Err(SandboxError::BadRequest(
            "Path traversal not allowed".into(),
        ));
    }

    Ok(full_path)
}

pub async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "healthy".into(),
    })
}

pub async fn execute_bash(
    State(state): State<Arc<AppState>>,
    Json(req): Json<BashRequest>,
) -> Result<Json<ExecutionResult>, SandboxError> {
    let chat_dir = get_chat_dir(&state.config.scratch_dir, &req.chat_id)?;
    fs::create_dir_all(&chat_dir)
        .await
        .map_err(|e| SandboxError::Internal(format!("Cannot create chat dir: {e}")))?;

    let result = run_command(&state.config, &chat_dir, &["bash", "-c", &req.command])
        .await
        .map_err(|e| SandboxError::Internal(e))?;

    versioning::commit(
        &chat_dir,
        &req.chat_id,
        &state.git_locks,
        &versioning::commit_message("run_bash", None),
    )
    .await;

    Ok(Json(result))
}

pub async fn execute_python(
    State(state): State<Arc<AppState>>,
    Json(req): Json<PythonRequest>,
) -> Result<Json<ExecutionResult>, SandboxError> {
    let chat_dir = get_chat_dir(&state.config.scratch_dir, &req.chat_id)?;
    fs::create_dir_all(&chat_dir)
        .await
        .map_err(|e| SandboxError::Internal(format!("Cannot create chat dir: {e}")))?;

    let script_path = chat_dir.join("_script.py");
    fs::write(&script_path, &req.code)
        .await
        .map_err(|e| SandboxError::Internal(format!("Cannot write script: {e}")))?;

    let script_str = script_path.to_string_lossy().to_string();
    let result = run_command(&state.config, &chat_dir, &["python3", &script_str])
        .await
        .map_err(|e| SandboxError::Internal(e))?;

    // Clean up script file (best effort)
    let _ = fs::remove_file(&script_path).await;

    versioning::commit(
        &chat_dir,
        &req.chat_id,
        &state.git_locks,
        &versioning::commit_message("run_python", None),
    )
    .await;

    Ok(Json(result))
}

pub async fn write_file(
    State(state): State<Arc<AppState>>,
    Json(req): Json<FileWriteRequest>,
) -> Result<Json<FileResult>, SandboxError> {
    let chat_dir = get_chat_dir(&state.config.scratch_dir, &req.chat_id)?;
    fs::create_dir_all(&chat_dir)
        .await
        .map_err(|e| SandboxError::Internal(format!("Cannot create chat dir: {e}")))?;

    let file_path = validate_path(&chat_dir, &req.path)?;

    if let Some(parent) = file_path.parent() {
        fs::create_dir_all(parent)
            .await
            .map_err(|e| SandboxError::Internal(format!("Cannot create directories: {e}")))?;
    }

    let byte_count = req.content.len();
    fs::write(&file_path, &req.content)
        .await
        .map_err(|e| SandboxError::Internal(format!("Cannot write file: {e}")))?;

    versioning::commit(
        &chat_dir,
        &req.chat_id,
        &state.git_locks,
        &versioning::commit_message("write_file", Some(&req.path)),
    )
    .await;

    Ok(Json(FileResult {
        content: format!("File written successfully ({byte_count} bytes)"),
        path: req.path,
    }))
}

pub async fn write_file_binary(
    State(state): State<Arc<AppState>>,
    Json(req): Json<BinaryFileWriteRequest>,
) -> Result<Json<FileResult>, SandboxError> {
    use base64::Engine;

    let chat_dir = get_chat_dir(&state.config.scratch_dir, &req.chat_id)?;
    fs::create_dir_all(&chat_dir)
        .await
        .map_err(|e| SandboxError::Internal(format!("Cannot create chat dir: {e}")))?;

    let file_path = validate_path(&chat_dir, &req.path)?;

    if let Some(parent) = file_path.parent() {
        fs::create_dir_all(parent)
            .await
            .map_err(|e| SandboxError::Internal(format!("Cannot create directories: {e}")))?;
    }

    let decoded_bytes = base64::engine::general_purpose::STANDARD
        .decode(&req.content_base64)
        .map_err(|e| SandboxError::BadRequest(format!("Invalid base64 content: {e}")))?;

    let byte_count = decoded_bytes.len();
    fs::write(&file_path, &decoded_bytes)
        .await
        .map_err(|e| SandboxError::Internal(format!("Cannot write file: {e}")))?;

    versioning::commit(
        &chat_dir,
        &req.chat_id,
        &state.git_locks,
        &versioning::commit_message("write_file", Some(&req.path)),
    )
    .await;

    Ok(Json(FileResult {
        content: format!("Binary file written successfully ({byte_count} bytes)"),
        path: req.path,
    }))
}

pub async fn edit_file(
    State(state): State<Arc<AppState>>,
    Json(req): Json<FileEditRequest>,
) -> Result<Json<FileResult>, SandboxError> {
    let chat_dir = get_chat_dir(&state.config.scratch_dir, &req.chat_id)?;

    let file_path = validate_path(&chat_dir, &req.path)?;

    if !file_path.exists() {
        return Err(SandboxError::NotFound(format!(
            "File not found: {}",
            req.path
        )));
    }

    let raw = fs::read(&file_path)
        .await
        .map_err(|e| SandboxError::Internal(format!("Cannot read file: {e}")))?;

    let content = String::from_utf8(raw).map_err(|_| {
        SandboxError::BadRequest(format!(
            "File {} is binary and cannot be edited with edit_file. Use run_python or run_bash to modify it.",
            req.path
        ))
    })?;

    if req.old_string.is_empty() {
        return Err(SandboxError::BadRequest(
            "old_string must not be empty".into(),
        ));
    }

    let match_count = content.matches(&req.old_string).count();
    if match_count == 0 {
        return Err(SandboxError::BadRequest(
            "old_string not found in file. Use read_file to check the exact content, then retry."
                .into(),
        ));
    }
    let replace_all = req.replace_all.unwrap_or(false);
    if match_count > 1 && !replace_all {
        return Err(SandboxError::BadRequest(format!(
            "old_string matches {match_count} locations. Either include more surrounding context to make it unique, or pass replace_all=true to replace every occurrence."
        )));
    }

    let replacements = if replace_all { match_count } else { 1 };
    let updated = if replace_all {
        content.replace(&req.old_string, &req.new_string)
    } else {
        content.replacen(&req.old_string, &req.new_string, 1)
    };

    fs::write(&file_path, updated)
        .await
        .map_err(|e| SandboxError::Internal(format!("Cannot write file: {e}")))?;

    versioning::commit(
        &chat_dir,
        &req.chat_id,
        &state.git_locks,
        &versioning::commit_message("edit_file", Some(&req.path)),
    )
    .await;

    Ok(Json(FileResult {
        content: format!(
            "File edited successfully ({replacements} replacement{}). Call present_artifact if the user should see the updated file.",
            if replacements == 1 { "" } else { "s" }
        ),
        path: req.path,
    }))
}

pub async fn read_file(
    State(state): State<Arc<AppState>>,
    Json(req): Json<FileReadRequest>,
) -> Result<Json<FileResult>, SandboxError> {
    let chat_dir = get_chat_dir(&state.config.scratch_dir, &req.chat_id)?;
    fs::create_dir_all(&chat_dir)
        .await
        .map_err(|e| SandboxError::Internal(format!("Cannot create chat dir: {e}")))?;

    let file_path = validate_path(&chat_dir, &req.path)?;

    if !file_path.exists() {
        return Err(SandboxError::NotFound(format!(
            "File not found: {}",
            req.path
        )));
    }

    let raw = fs::read_to_string(&file_path)
        .await
        .map_err(|e| SandboxError::Internal(format!("Cannot read file: {e}")))?;

    let lines: Vec<&str> = raw.lines().collect();
    let total = lines.len();

    const MAX_FULL_READ_BYTES: usize = 20_000; // ~20KB

    let has_range = req.start_line.is_some() || req.end_line.is_some();

    if !has_range && raw.len() > MAX_FULL_READ_BYTES {
        let size_kb = raw.len() / 1024;
        return Err(SandboxError::BadRequest(format!(
            "File is {size_kb}KB ({total} lines) which exceeds the {}KB limit for a full read. \
             Use start_line/end_line to read a specific range, or use run_bash with grep to find relevant sections first.",
            MAX_FULL_READ_BYTES / 1024
        )));
    }

    let start = req.start_line.unwrap_or(1).max(1).min(total.max(1));
    let end = req.end_line.unwrap_or(total).max(1).min(total.max(1));

    let content = if total == 0 {
        String::new()
    } else if start > end {
        String::new()
    } else {
        lines[start - 1..=end - 1]
            .iter()
            .enumerate()
            .map(|(i, line)| format!("{} | {}", start + i, line))
            .collect::<Vec<_>>()
            .join("\n")
    };

    Ok(Json(FileResult {
        content: truncate_output(&content),
        path: req.path,
    }))
}

pub async fn file_stat(
    State(state): State<Arc<AppState>>,
    Json(req): Json<FileStatRequest>,
) -> Result<Json<FileStatResponse>, SandboxError> {
    let chat_dir = get_chat_dir(&state.config.scratch_dir, &req.chat_id)?;

    if !chat_dir.exists() {
        return Ok(Json(FileStatResponse {
            path: req.path,
            size_bytes: 0,
            content_type: String::new(),
            exists: false,
            version: None,
        }));
    }

    let file_path = validate_path(&chat_dir, &req.path)?;

    if !file_path.exists() {
        return Ok(Json(FileStatResponse {
            path: req.path,
            size_bytes: 0,
            content_type: String::new(),
            exists: false,
            version: None,
        }));
    }

    let metadata = fs::metadata(&file_path)
        .await
        .map_err(|e| SandboxError::Internal(format!("Cannot stat file: {e}")))?;

    let content_type = mime_guess::from_path(&file_path)
        .first_or_octet_stream()
        .to_string();

    // present_artifact pins the artifact to the version being presented, so a
    // later edit never changes what an earlier artifact card shows.
    versioning::ensure_repo(&chat_dir).await;
    let version = versioning::head_version(&chat_dir, &req.path).await;

    Ok(Json(FileStatResponse {
        path: req.path,
        size_bytes: metadata.len(),
        content_type,
        exists: true,
        version,
    }))
}

pub async fn file_versions(
    State(state): State<Arc<AppState>>,
    Json(req): Json<FileVersionsRequest>,
) -> Result<Json<Vec<FileVersionResponse>>, SandboxError> {
    let chat_dir = get_chat_dir(&state.config.scratch_dir, &req.chat_id)?;

    if !chat_dir.exists() {
        return Err(SandboxError::NotFound("Chat directory not found".into()));
    }

    let file_path = validate_path(&chat_dir, &req.path)?;
    if !file_path.exists() {
        return Err(SandboxError::NotFound(format!(
            "File not found: {}",
            req.path
        )));
    }

    let versions = versioning::list_versions(&chat_dir, &req.path)
        .await
        .map_err(SandboxError::Internal)?;

    Ok(Json(
        versions
            .into_iter()
            .map(|v| FileVersionResponse {
                sha: v.sha,
                timestamp: v.timestamp,
                message: v.message,
            })
            .collect(),
    ))
}

pub async fn download_file(
    State(state): State<Arc<AppState>>,
    Query(req): Query<FileDownloadQuery>,
) -> Result<Response, SandboxError> {
    let chat_dir = get_chat_dir(&state.config.scratch_dir, &req.chat_id)?;

    if !chat_dir.exists() {
        return Err(SandboxError::NotFound("Chat directory not found".into()));
    }

    let file_path = validate_path(&chat_dir, &req.path)?;

    let body = if let Some(version) = &req.version {
        versioning::read_version(&chat_dir, version, &req.path)
            .await
            .map_err(SandboxError::NotFound)?
    } else {
        if !file_path.exists() {
            return Err(SandboxError::NotFound(format!(
                "File not found: {}",
                req.path
            )));
        }
        fs::read(&file_path)
            .await
            .map_err(|e| SandboxError::Internal(format!("Cannot read file: {e}")))?
    };

    let content_type = mime_guess::from_path(&file_path)
        .first_or_octet_stream()
        .to_string();

    Ok(Response::builder()
        .header(header::CONTENT_TYPE, content_type)
        // Artifact bytes may be read by sandboxed iframe previews (unique
        // origin), so allow cross-origin reads; auth is path-based.
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .body(axum::body::Body::from(body))
        .unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_get_chat_dir_sanitizes() {
        let scratch = PathBuf::from("/scratch");
        let dir = get_chat_dir(&scratch, "abc-123").unwrap();
        assert_eq!(dir, PathBuf::from("/scratch/abc-123"));
    }

    #[test]
    fn test_get_chat_dir_strips_traversal() {
        let scratch = PathBuf::from("/scratch");
        let dir = get_chat_dir(&scratch, "../etc/passwd").unwrap();
        // ".." is removed, "/" is removed → "etcpasswd"
        assert_eq!(dir, PathBuf::from("/scratch/etcpasswd"));
    }

    #[test]
    fn test_get_chat_dir_empty_after_sanitize() {
        let scratch = PathBuf::from("/scratch");
        let result = get_chat_dir(&scratch, "../../");
        assert!(result.is_err());
    }

    #[test]
    fn test_truncate_output_short() {
        use crate::executor::truncate_output;
        let short = "hello".to_string();
        assert_eq!(truncate_output(&short), "hello");
    }

    #[test]
    fn test_truncate_output_long() {
        use crate::executor::truncate_output;
        let long = "x".repeat(200_000);
        let result = truncate_output(&long);
        assert!(result.len() < 200_000);
        assert!(result.ends_with("... (output truncated)"));
    }
}
