use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use serde::Deserialize;
use tracing::{error, info};

use crate::models::AtlassianWebhookEvent;
use crate::sync::SyncManager;

#[derive(Clone)]
struct RoutesState {
    sync_manager: Arc<SyncManager>,
}

#[derive(Debug, Deserialize)]
struct WebhookQuery {
    source_id: String,
}

pub fn build_router(sync_manager: Arc<SyncManager>) -> Router {
    Router::new()
        .route("/webhook", post(handle_webhook))
        .with_state(RoutesState { sync_manager })
}

async fn handle_webhook(
    State(state): State<RoutesState>,
    Query(query): Query<WebhookQuery>,
    Json(event): Json<AtlassianWebhookEvent>,
) -> StatusCode {
    let source_id = query.source_id;
    info!(
        "Received webhook event '{}' for source {}",
        event.webhook_event, source_id
    );

    let sync_manager = state.sync_manager.clone();
    tokio::spawn(async move {
        if let Err(e) = sync_manager.handle_webhook_event(&source_id, event).await {
            error!(
                "Failed to handle webhook event for source {}: {}",
                source_id, e
            );
        }
    });

    StatusCode::OK
}
