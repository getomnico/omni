use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use axum::http::StatusCode;
use omni_connector_sdk::{
    ActionDefinition, ActionResponse, Connector, ServiceCredential, Source, SourceType,
    SyncContext, SyncType,
};
use serde_json::{json, Value as JsonValue};

use crate::models::FileSystemConfig;
use crate::{sync, watcher};

#[derive(Default)]
pub struct FileSystemConnector;

#[async_trait]
impl Connector for FileSystemConnector {
    type Config = FileSystemConfig;
    type Credentials = JsonValue;
    type State = JsonValue;

    fn name(&self) -> &'static str {
        "filesystem"
    }

    fn version(&self) -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    fn source_types(&self) -> Vec<SourceType> {
        vec![SourceType::LocalFiles, SourceType::FileSystem]
    }

    fn sync_modes(&self) -> Vec<SyncType> {
        vec![SyncType::Full, SyncType::Realtime]
    }

    fn display_name(&self) -> String {
        "File System".to_string()
    }

    fn description(&self) -> Option<String> {
        Some("Index files and documents from a local filesystem".to_string())
    }

    fn read_only(&self) -> bool {
        true
    }

    fn requires_credentials(&self) -> bool {
        false
    }

    fn actions(&self) -> Vec<ActionDefinition> {
        vec![ActionDefinition {
            name: "validate_path".to_string(),
            description: "Validate that the configured filesystem path exists and is a directory"
                .to_string(),
            mode: omni_connector_sdk::ActionMode::Read,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "base_path": { "type": "string" }
                },
                "required": ["base_path"]
            }),
            source_types: Vec::new(),
            admin_only: false,
        }]
    }

    async fn sync(
        &self,
        source: Source,
        _credentials: Option<ServiceCredential>,
        _state: Option<JsonValue>,
        ctx: SyncContext,
    ) -> Result<()> {
        let source_name = source.name.clone();
        let source_config: FileSystemConfig = serde_json::from_value(source.config)
            .context("Failed to decode filesystem source config")?;
        match ctx.sync_mode() {
            SyncType::Full | SyncType::Incremental => {
                sync::run_sync(source_name, source_config, ctx).await
            }
            SyncType::Realtime => watcher::run_realtime(source_name, source_config, ctx).await,
        }
    }

    async fn execute_action(
        &self,
        action: &str,
        params: JsonValue,
        _credentials: Option<ServiceCredential>,
    ) -> Result<axum::response::Response> {
        match action {
            "validate_path" => {
                let base_path = params
                    .get("base_path")
                    .and_then(|value| value.as_str())
                    .ok_or_else(|| anyhow!("Missing 'base_path' in params"))?;

                let path = std::path::Path::new(base_path);
                Ok(ActionResponse::success(json!({
                    "exists": path.exists(),
                    "is_directory": path.is_dir(),
                    "valid": path.exists() && path.is_dir()
                }))
                .into_response())
            }
            other => Ok(ActionResponse::not_supported(other)
                .into_response_with_status(StatusCode::NOT_FOUND)),
        }
    }
}
