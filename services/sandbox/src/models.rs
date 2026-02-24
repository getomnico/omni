use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct BashRequest {
    pub command: String,
    pub chat_id: String,
}

#[derive(Debug, Deserialize)]
pub struct PythonRequest {
    pub code: String,
    pub chat_id: String,
}

#[derive(Debug, Deserialize)]
pub struct FileWriteRequest {
    pub path: String,
    pub content: String,
    pub chat_id: String,
}

#[derive(Debug, Deserialize)]
pub struct FileReadRequest {
    pub path: String,
    pub chat_id: String,
}

#[derive(Debug, Serialize)]
pub struct ExecutionResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

#[derive(Debug, Serialize)]
pub struct FileResult {
    pub content: String,
    pub path: String,
}

#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub status: String,
}
