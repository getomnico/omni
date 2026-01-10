use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use shared::models::SourceType;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectorManifest {
    pub name: String,
    pub version: String,
    pub sync_modes: Vec<String>,
    #[serde(default)]
    pub actions: Vec<ActionDefinition>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionDefinition {
    pub name: String,
    pub description: String,
    pub parameters: JsonValue,
}

// SyncRequest and SyncResponse are imported from shared crate
pub use shared::models::{SyncRequest, SyncResponse};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionRequest {
    pub action: String,
    pub params: JsonValue,
    pub credentials: JsonValue,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionResponse {
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<JsonValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TriggerType {
    Scheduled,
    Manual,
    Webhook,
}

impl std::fmt::Display for TriggerType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TriggerType::Scheduled => write!(f, "scheduled"),
            TriggerType::Manual => write!(f, "manual"),
            TriggerType::Webhook => write!(f, "webhook"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncProgress {
    pub sync_run_id: String,
    pub source_id: String,
    pub status: String,
    pub documents_scanned: i32,
    pub documents_processed: i32,
    pub documents_updated: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduleInfo {
    pub source_id: String,
    pub source_name: String,
    pub source_type: String,
    pub sync_interval_seconds: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_sync_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_sync_at: Option<String>,
    pub sync_status: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectorInfo {
    pub source_type: SourceType,
    pub url: String,
    pub healthy: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manifest: Option<ConnectorManifest>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriggerSyncRequest {
    pub source_id: String,
    #[serde(default)]
    pub sync_mode: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriggerSyncResponse {
    pub sync_run_id: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecuteActionRequest {
    pub source_id: String,
    pub action: String,
    pub params: JsonValue,
}

// ============================================================================
// SDK Models - Used by connectors to communicate with connector-manager
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SdkEmitEventRequest {
    pub sync_run_id: String,
    pub source_id: String,
    pub event: shared::models::ConnectorEvent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SdkStoreContentRequest {
    pub sync_run_id: String,
    pub content: String,
    #[serde(default)]
    pub content_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SdkStoreContentResponse {
    pub content_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SdkCompleteRequest {
    #[serde(default)]
    pub documents_scanned: Option<i32>,
    #[serde(default)]
    pub documents_updated: Option<i32>,
    #[serde(default)]
    pub new_state: Option<JsonValue>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SdkFailRequest {
    pub error: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SdkIncrementScannedRequest {
    #[serde(default = "default_count")]
    pub count: i32,
}

fn default_count() -> i32 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SdkStatusResponse {
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SdkCreateSyncRequest {
    pub source_id: String,
    pub sync_type: shared::models::SyncType,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SdkCreateSyncResponse {
    pub sync_run_id: String,
}

// ============================================================================
// SDK Cancel Sync
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SdkCancelSyncRequest {
    pub sync_run_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SdkCancelSyncResponse {
    pub success: bool,
}

// ============================================================================
// SDK User Email
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SdkUserEmailResponse {
    pub email: String,
}

// ============================================================================
// SDK Webhook Notification
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SdkWebhookNotification {
    pub source_id: String,
    pub event_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SdkWebhookResponse {
    pub sync_run_id: String,
}

// ============================================================================
// SDK Webhook Channel Management
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SdkSaveWebhookChannelRequest {
    pub source_id: String,
    pub channel_id: String,
    pub resource_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource_uri: Option<String>,
    pub webhook_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SdkWebhookChannel {
    pub id: String,
    pub source_id: String,
    pub channel_id: String,
    pub resource_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource_uri: Option<String>,
    pub webhook_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SdkExpiringWebhookChannelsRequest {
    pub hours_ahead: i64,
}
