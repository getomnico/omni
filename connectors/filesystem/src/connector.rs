use anyhow::{anyhow, Result};
use async_trait::async_trait;
use omni_connector_sdk::{ActionDefinition, ActionResponse, Connector, SourceType, SyncContext};
use serde_json::{json, Value as JsonValue};

use crate::models::FileSystemConfig;
use crate::sync;

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
            mode: "read".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "base_path": { "type": "string" }
                },
                "required": ["base_path"]
            }),
        }]
    }

    async fn sync(
        &self,
        source_config: FileSystemConfig,
        _credentials: JsonValue,
        _state: Option<JsonValue>,
        ctx: SyncContext,
    ) -> Result<()> {
        let source_name = ctx.sdk_client().get_source(ctx.source_id()).await?.name;
        sync::run_sync(source_name, source_config, ctx).await
    }

    async fn execute_action(
        &self,
        action: &str,
        params: JsonValue,
        _credentials: JsonValue,
    ) -> Result<ActionResponse> {
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
                })))
            }
            other => Ok(ActionResponse::not_supported(other)),
        }
    }
}
