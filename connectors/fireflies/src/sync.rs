use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use dashmap::DashMap;
use shared::models::{ServiceProvider, SourceType, SyncRequest};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tracing::{error, info};

use crate::client::FirefliesClient;
use shared::SdkClient;

pub struct SyncManager {
    sdk_client: SdkClient,
    client: FirefliesClient,
    active_syncs: DashMap<String, Arc<AtomicBool>>,
}

impl SyncManager {
    pub fn new(sdk_client: SdkClient) -> Self {
        Self {
            sdk_client,
            client: FirefliesClient::new(),
            active_syncs: DashMap::new(),
        }
    }

    pub fn cancel_sync(&self, sync_run_id: &str) -> bool {
        if let Some(cancelled) = self.active_syncs.get(sync_run_id) {
            cancelled.store(true, Ordering::SeqCst);
            true
        } else {
            false
        }
    }

    pub async fn sync_source(&mut self, request: SyncRequest) -> Result<()> {
        let sync_run_id = &request.sync_run_id;
        let source_id = &request.source_id;

        info!(
            "Starting sync for source: {} (sync_run_id: {})",
            source_id, sync_run_id
        );

        let source = self
            .sdk_client
            .get_source(source_id)
            .await
            .context("Failed to fetch source via SDK")?;

        if !source.is_active {
            let err_msg = format!("Source is not active: {}", source_id);
            self.sdk_client.fail(sync_run_id, &err_msg).await?;
            return Err(anyhow!(err_msg));
        }

        if source.source_type != SourceType::Fireflies {
            let err_msg = format!(
                "Invalid source type for Fireflies connector: {:?}",
                source.source_type
            );
            self.sdk_client.fail(sync_run_id, &err_msg).await?;
            return Err(anyhow!(err_msg));
        }

        let creds = match self.sdk_client.get_credentials(source_id).await {
            Ok(c) => c,
            Err(e) => {
                self.sdk_client.fail(sync_run_id, &e.to_string()).await?;
                return Err(e.into());
            }
        };

        if creds.provider != ServiceProvider::Fireflies {
            let err_msg = format!("Expected Fireflies credentials, found {:?}", creds.provider);
            self.sdk_client.fail(sync_run_id, &err_msg).await?;
            return Err(anyhow!(err_msg));
        }

        let api_key = creds
            .credentials
            .get("api_key")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("Missing api_key in credentials"))?
            .to_string();

        if let Err(e) = self.client.test_connection(&api_key).await {
            let err_msg = format!("Fireflies connection test failed: {}", e);
            self.sdk_client.fail(sync_run_id, &err_msg).await?;
            return Err(anyhow!(err_msg));
        }

        let cancelled = Arc::new(AtomicBool::new(false));
        self.active_syncs
            .insert(sync_run_id.to_string(), cancelled.clone());

        let is_full_sync = request.sync_mode == "full";
        let from_date = if is_full_sync {
            None
        } else {
            request.last_sync_at.clone()
        };

        info!(
            "Performing {} sync for source: {}",
            if is_full_sync { "full" } else { "incremental" },
            source.name
        );

        let result = self
            .execute_sync(
                &api_key,
                source_id,
                sync_run_id,
                from_date.as_deref(),
                &cancelled,
            )
            .await;

        if cancelled.load(Ordering::SeqCst) {
            info!("Sync {} was cancelled", sync_run_id);
            let _ = self.sdk_client.cancel(sync_run_id).await;
            self.active_syncs.remove(sync_run_id);
            return Ok(());
        }

        self.active_syncs.remove(sync_run_id);

        match result {
            Ok(total_processed) => {
                info!(
                    "Sync completed for source {}: {} transcripts processed",
                    source.name, total_processed
                );
                let new_state = serde_json::json!({ "last_sync_time": Utc::now().to_rfc3339() });
                self.sdk_client
                    .complete(
                        sync_run_id,
                        total_processed as i32,
                        total_processed as i32,
                        Some(new_state),
                    )
                    .await?;
                Ok(())
            }
            Err(e) => {
                error!("Sync failed for source {}: {}", source.name, e);
                self.sdk_client.fail(sync_run_id, &e.to_string()).await?;
                Err(e)
            }
        }
    }

    async fn execute_sync(
        &self,
        api_key: &str,
        source_id: &str,
        sync_run_id: &str,
        from_date: Option<&str>,
        cancelled: &AtomicBool,
    ) -> Result<u32> {
        let transcripts = self
            .client
            .fetch_all_transcripts(api_key, from_date)
            .await?;

        let total = transcripts.len();
        info!("Fetched {} transcripts to process", total);

        let mut processed = 0u32;

        for transcript in &transcripts {
            if cancelled.load(Ordering::SeqCst) {
                info!("Sync cancelled, stopping after {} transcripts", processed);
                return Ok(processed);
            }

            let content = transcript.generate_content();

            let content_id = self
                .sdk_client
                .store_content(sync_run_id, &content)
                .await
                .context("Failed to store transcript content")?;

            let event = transcript.to_connector_event(
                sync_run_id.to_string(),
                source_id.to_string(),
                content_id,
            );

            self.sdk_client
                .emit_event(sync_run_id, source_id, event)
                .await
                .context("Failed to emit connector event")?;

            processed += 1;

            if processed % 10 == 0 {
                let _ = self.sdk_client.increment_scanned(sync_run_id, 10).await;
            }
        }

        if processed % 10 != 0 {
            let _ = self
                .sdk_client
                .increment_scanned(sync_run_id, (processed % 10) as i32)
                .await;
        }

        Ok(processed)
    }
}
