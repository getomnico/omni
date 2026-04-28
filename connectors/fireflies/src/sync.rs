use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use omni_connector_sdk::SyncContext;
use serde_json::json;
use shared::models::SyncType;
use tracing::info;

use crate::client::FirefliesClient;
use crate::connector::FirefliesState;

pub async fn run_sync(
    client: &FirefliesClient,
    api_key: &str,
    state: Option<FirefliesState>,
    ctx: SyncContext,
) -> Result<()> {
    let sync_run_id = ctx.sync_run_id().to_string();
    let source_id = ctx.source_id().to_string();

    info!(
        "Starting sync for source: {} (sync_run_id: {})",
        source_id, sync_run_id
    );

    client
        .test_connection(api_key)
        .await
        .map_err(|e| anyhow!("Fireflies connection test failed: {}", e))?;

    let from_date = match ctx.sync_mode() {
        SyncType::Full => None,
        _ => state.and_then(|s| s.last_sync_time),
    };

    info!(
        "Performing {} sync for source: {}",
        if from_date.is_none() {
            "full"
        } else {
            "incremental"
        },
        source_id
    );

    let transcripts = client
        .fetch_all_transcripts(api_key, from_date.as_deref())
        .await?;

    let total = transcripts.len();
    info!("Fetched {} transcripts to process", total);

    let mut processed = 0u32;

    for transcript in &transcripts {
        if ctx.is_cancelled() {
            info!("Sync cancelled, stopping after {} transcripts", processed);
            return Ok(());
        }

        let content = transcript.generate_content();

        let content_id = ctx
            .store_content(&content)
            .await
            .context("Failed to store transcript content")?;

        let event =
            transcript.to_connector_event(sync_run_id.clone(), source_id.clone(), content_id);

        ctx.emit_event(event)
            .await
            .context("Failed to emit connector event")?;

        processed += 1;

        if processed.is_multiple_of(10) {
            let _ = ctx.increment_scanned(10).await;
        }
    }

    if !processed.is_multiple_of(10) {
        let _ = ctx.increment_scanned((processed % 10) as i32).await;
    }

    info!(
        "Sync completed for source {}: {} transcripts processed",
        source_id, processed
    );

    let new_state = json!({ "last_sync_time": Utc::now().to_rfc3339() });
    ctx.save_connector_state(new_state).await?;
    ctx.complete().await?;

    Ok(())
}
