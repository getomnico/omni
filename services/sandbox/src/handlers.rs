use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::extract::State;
use axum::Json;
use tokio::fs;

use crate::executor::{run_command, truncate_output};
use crate::models::*;
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

    Ok(Json(FileResult {
        content: format!("File written successfully ({byte_count} bytes)"),
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

    let content = fs::read_to_string(&file_path)
        .await
        .map_err(|e| SandboxError::Internal(format!("Cannot read file: {e}")))?;

    Ok(Json(FileResult {
        content: truncate_output(&content),
        path: req.path,
    }))
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
