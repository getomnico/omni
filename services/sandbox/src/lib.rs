pub mod executor;
pub mod handlers;
pub mod models;

use std::path::PathBuf;
use std::sync::Arc;

use axum::http::StatusCode;
use axum::response::{IntoResponse, Json};
use axum::routing::{get, post};
use axum::Router;
use tower_http::trace::TraceLayer;

#[derive(Debug, Clone)]
pub struct SandboxConfig {
    pub port: u16,
    pub scratch_dir: PathBuf,
    pub execution_timeout: u64,
    pub sandbox_enabled: bool,
    pub sandbox_exec_path: Option<String>,
}

impl SandboxConfig {
    pub fn from_env() -> Self {
        Self {
            port: std::env::var("PORT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(8090),
            scratch_dir: PathBuf::from(
                std::env::var("SCRATCH_DIR").unwrap_or_else(|_| "/scratch".into()),
            ),
            execution_timeout: std::env::var("EXECUTION_TIMEOUT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(30),
            sandbox_enabled: std::env::var("SANDBOX_ENABLED")
                .unwrap_or_else(|_| "true".into())
                .to_lowercase()
                == "true",
            sandbox_exec_path: std::env::var("SANDBOX_EXEC_PATH").ok(),
        }
    }
}

#[derive(Debug)]
pub struct AppState {
    pub config: SandboxConfig,
}

#[derive(Debug, thiserror::Error)]
pub enum SandboxError {
    #[error("Bad request: {0}")]
    BadRequest(String),
    #[error("Not found: {0}")]
    NotFound(String),
    #[error("Internal error: {0}")]
    Internal(String),
}

impl IntoResponse for SandboxError {
    fn into_response(self) -> axum::response::Response {
        let (status, message) = match &self {
            SandboxError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg.clone()),
            SandboxError::NotFound(msg) => (StatusCode::NOT_FOUND, msg.clone()),
            SandboxError::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg.clone()),
        };
        let body = serde_json::json!({ "detail": message });
        (status, Json(body)).into_response()
    }
}

pub fn create_app(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/health", get(handlers::health))
        .route("/execute/bash", post(handlers::execute_bash))
        .route("/execute/python", post(handlers::execute_python))
        .route("/files/write", post(handlers::write_file))
        .route("/files/read", post(handlers::read_file))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

async fn landlock_smoke_test(config: &SandboxConfig) -> anyhow::Result<()> {
    let test_dir = config.scratch_dir.join("_landlock_test");
    tokio::fs::create_dir_all(&test_dir).await?;

    let sandbox_exec = config
        .sandbox_exec_path
        .as_deref()
        .unwrap_or("sandbox-exec");

    let output = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        tokio::process::Command::new(sandbox_exec)
            .arg(test_dir.to_str().unwrap_or_default())
            .arg("--")
            .arg("echo")
            .arg("ok")
            .output(),
    )
    .await??;

    let _ = tokio::fs::remove_dir(&test_dir).await;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!(
            "Landlock smoke test failed (exit {}): {}",
            output.status.code().unwrap_or(-1),
            stderr
        );
    }

    tracing::info!("Landlock smoke test passed");
    Ok(())
}

pub async fn run_server() -> anyhow::Result<()> {
    let _ = dotenvy::dotenv();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let config = SandboxConfig::from_env();
    tracing::info!(
        port = config.port,
        scratch_dir = %config.scratch_dir.display(),
        sandbox_enabled = config.sandbox_enabled,
        timeout = config.execution_timeout,
        "Starting sandbox service"
    );

    // Ensure scratch directory exists
    tokio::fs::create_dir_all(&config.scratch_dir).await?;

    if config.sandbox_enabled {
        landlock_smoke_test(&config).await?;
    } else {
        tracing::info!("Sandbox isolation is disabled");
    }

    let state = Arc::new(AppState {
        config: config.clone(),
    });
    let app = create_app(state);

    let addr = format!("0.0.0.0:{}", config.port);
    tracing::info!("Listening on {addr}");
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
