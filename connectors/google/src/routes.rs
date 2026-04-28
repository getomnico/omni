//! Connector-specific HTTP routes that live outside the SDK protocol
//! surface: Google Drive push-notification webhook.

use std::sync::Arc;

use axum::{
    extract::State,
    http::{header::HeaderMap, StatusCode},
    routing::post,
    Router,
};
use tracing::{debug, error, info, warn};

use crate::admin::AdminClient;
use crate::models::WebhookNotification;

use crate::sync::SyncManager;

#[derive(Clone)]
pub struct RoutesState {
    pub sync_manager: Arc<SyncManager>,
    pub admin_client: Arc<AdminClient>,
}

pub fn build_router(sync_manager: Arc<SyncManager>, admin_client: Arc<AdminClient>) -> Router {
    let state = RoutesState {
        sync_manager,
        admin_client,
    };
    Router::new()
        .route("/webhook", post(handle_webhook))
        .with_state(state)
}

// ---------------------------------------------------------------------------
// /webhook — Google Drive push notifications
// ---------------------------------------------------------------------------

async fn handle_webhook(
    State(state): State<RoutesState>,
    headers: HeaderMap,
) -> Result<StatusCode, StatusCode> {
    debug!("Received webhook notification");

    let notification = match WebhookNotification::from_headers(&headers) {
        Some(notification) => notification,
        None => {
            warn!("Failed to parse webhook notification from headers");
            return Err(StatusCode::BAD_REQUEST);
        }
    };

    info!(
        "Processing webhook notification: channel_id={}, resource_state={}, source_id={:?}",
        notification.channel_id, notification.resource_state, notification.source_id
    );

    let sync_manager = state.sync_manager.clone();
    let notification_clone = notification.clone();

    tokio::spawn(async move {
        if let Err(e) = sync_manager
            .handle_webhook_notification(notification_clone)
            .await
        {
            error!("Failed to handle webhook notification: {}", e);
        }
    });

    Ok(StatusCode::OK)
}
