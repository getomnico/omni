use anyhow::{anyhow, Result};
use futures::stream::StreamExt;
use redis::Client as RedisClient;
use shared::models::{ConnectorEvent, SyncType};
use tracing::{debug, error, info, warn};

use crate::auth::AtlassianCredentials;
use crate::client::AtlassianClient;
use crate::models::{ConfluencePage, ConfluencePageStatus, ConfluenceSpace};
use crate::sync::SyncState;
use shared::SdkClient;

pub struct ConfluenceProcessor {
    client: AtlassianClient,
    sdk_client: SdkClient,
    sync_state: SyncState,
}

impl ConfluenceProcessor {
    pub fn new(sdk_client: SdkClient, redis_client: RedisClient) -> Self {
        Self {
            client: AtlassianClient::new(),
            sdk_client,
            sync_state: SyncState::new(redis_client),
        }
    }

    pub async fn sync_all_spaces(
        &mut self,
        creds: &AtlassianCredentials,
        source_id: &str,
        sync_run_id: &str,
    ) -> Result<u32> {
        self.sync_spaces_internal(creds, source_id, sync_run_id, SyncType::Full)
            .await
    }

    pub async fn sync_all_spaces_incremental(
        &mut self,
        creds: &AtlassianCredentials,
        source_id: &str,
        sync_run_id: &str,
    ) -> Result<u32> {
        self.sync_spaces_internal(creds, source_id, sync_run_id, SyncType::Incremental)
            .await
    }

    async fn sync_spaces_internal(
        &mut self,
        creds: &AtlassianCredentials,
        source_id: &str,
        sync_run_id: &str,
        sync_type: SyncType,
    ) -> Result<u32> {
        let sync_type_str = match sync_type {
            SyncType::Full => "full",
            SyncType::Incremental => "incremental",
        };
        info!(
            "Starting {} Confluence sync for source: {} (sync_run_id: {})",
            sync_type_str, source_id, sync_run_id
        );

        let spaces = self.get_accessible_spaces(creds).await?;
        let mut total_pages_processed = 0;

        for space in spaces {
            info!(
                "Syncing Confluence space: {} [key={}, id={}]",
                space.name, space.key, space.id
            );

            match self
                .sync_space_pages(creds, source_id, sync_run_id, &space.id)
                .await
            {
                Ok(pages_count) => {
                    total_pages_processed += pages_count;
                    info!("Synced {} pages from space: {}", pages_count, space.id);
                    // Update scanned count via SDK
                    if let Err(e) = self
                        .sdk_client
                        .increment_scanned(sync_run_id, pages_count as i32)
                        .await
                    {
                        error!("Failed to increment scanned count: {}", e);
                    }
                }
                Err(e) => {
                    error!("Failed to sync space {}: {}", space.id, e);
                }
            }
        }

        info!(
            "Completed Confluence sync. Total pages processed: {}",
            total_pages_processed
        );
        Ok(total_pages_processed)
    }

    async fn sync_space_pages(
        &mut self,
        creds: &AtlassianCredentials,
        source_id: &str,
        sync_run_id: &str,
        space_id: &str,
    ) -> Result<u32> {
        let mut total_pages = 0;
        let mut pages_batch = Vec::with_capacity(100);

        info!("Fetching pages for Confluence space {}", space_id);
        let mut pages_stream = self.client.get_confluence_pages(creds, space_id);

        while let Some(page_result) = pages_stream.next().await {
            let page = page_result?;
            pages_batch.push(page);

            if pages_batch.len() >= 100 {
                let count = self
                    .process_pages(pages_batch, source_id, sync_run_id, &creds.base_url)
                    .await?;
                total_pages += count;
                pages_batch = Vec::with_capacity(100);
            }
        }

        if !pages_batch.is_empty() {
            let count = self
                .process_pages(pages_batch, source_id, sync_run_id, &creds.base_url)
                .await?;
            total_pages += count;
        }

        info!(
            "Processed {} pages from Confluence space {}",
            total_pages, space_id
        );
        Ok(total_pages)
    }

    async fn get_accessible_spaces(
        &mut self,
        creds: &AtlassianCredentials,
    ) -> Result<Vec<ConfluenceSpace>> {
        let spaces = self.client.get_confluence_spaces(creds).await?;
        if spaces.is_empty() {
            debug!("No spaces found for Confluence instance {}", creds.base_url);
        }
        debug!("Found {} accessible Confluence spaces", spaces.len());
        Ok(spaces)
    }

    async fn process_pages(
        &self,
        pages: Vec<ConfluencePage>,
        source_id: &str,
        sync_run_id: &str,
        base_url: &str,
    ) -> Result<u32> {
        let mut count = 0;

        for page in pages {
            // Skip non-current pages (drafts, trashed, etc.)
            if page.status != ConfluencePageStatus::Current {
                debug!("Skipping page {} with status: {:?}", page.id, page.status);
                continue;
            }

            // Check if page version has changed
            let current_version = page.version.number;
            let should_process = match self
                .sync_state
                .get_confluence_page_version(source_id, &page.space_id, &page.id)
                .await
            {
                Ok(Some(last_version)) => {
                    if last_version != current_version {
                        debug!(
                            "Page {} has been updated (was version {}, now version {})",
                            page.title, last_version, current_version
                        );
                        true
                    } else {
                        debug!(
                            "Skipping page {} - version {} unchanged",
                            page.title, current_version
                        );
                        false
                    }
                }
                Ok(None) => {
                    debug!("Page {} is new, will process", page.title);
                    true
                }
                Err(e) => {
                    warn!(
                        "Failed to get sync state for page {}: {}, will process",
                        page.id, e
                    );
                    true
                }
            };

            if !should_process {
                continue;
            }

            // Skip pages without content
            let content = page.extract_plain_text();
            if content.trim().is_empty() {
                debug!("Skipping page {} without content", page.id);
                continue;
            }

            debug!(
                "Processing Confluence page: {} in space {} (content length: {} chars)",
                page.title,
                page.space_id,
                content.len()
            );

            // Store content via SDK
            let content_id = match self.sdk_client.store_content(sync_run_id, &content).await {
                Ok(id) => id,
                Err(e) => {
                    error!(
                        "Failed to store content via SDK for Confluence page {}: {}",
                        page.title, e
                    );
                    continue;
                }
            };

            let event = page.to_connector_event(
                sync_run_id.to_string(),
                source_id.to_string(),
                base_url,
                content_id,
            );

            // Emit event via SDK
            if let Err(e) = self
                .sdk_client
                .emit_event(sync_run_id, source_id, event)
                .await
            {
                error!(
                    "Failed to emit event for Confluence page {}: {}",
                    page.title, e
                );
                continue;
            }

            count += 1;

            // Update sync state
            if let Err(e) = self
                .sync_state
                .set_confluence_page_version(source_id, &page.space_id, &page.id, current_version)
                .await
            {
                warn!("Failed to update sync state for page {}: {}", page.id, e);
            }
        }

        Ok(count)
    }

    pub async fn sync_single_page(
        &mut self,
        creds: &AtlassianCredentials,
        source_id: &str,
        page_id: &str,
    ) -> Result<()> {
        info!("Syncing single Confluence page: {}", page_id);

        let expand = vec![
            "body.storage",
            "space",
            "version",
            "ancestors",
            "_links.webui",
        ];

        let page = self
            .client
            .get_confluence_page_by_id(creds, page_id, &expand)
            .await?;

        if page.status != ConfluencePageStatus::Current {
            warn!(
                "Page {} is not current (status: {:?}), skipping",
                page_id, page.status
            );
            return Ok(());
        }

        let content = page.extract_plain_text();
        if content.trim().is_empty() {
            warn!("Page {} has no content, skipping", page_id);
            return Ok(());
        }

        // Create sync run via SDK
        let sync_run_id = self
            .sdk_client
            .create_sync_run(source_id, SyncType::Incremental)
            .await
            .map_err(|e| anyhow!("Failed to create sync run via SDK: {}", e))?;

        let result: Result<()> = async {
            let content_id = self
                .sdk_client
                .store_content(&sync_run_id, &content)
                .await
                .map_err(|e| {
                    anyhow!(
                        "Failed to store content via SDK for Confluence page {}: {}",
                        page.title,
                        e
                    )
                })?;

            let event = page.to_connector_event(
                sync_run_id.clone(),
                source_id.to_string(),
                &creds.base_url,
                content_id,
            );
            self.sdk_client
                .emit_event(&sync_run_id, source_id, event)
                .await?;

            info!("Successfully queued page: {}", page.title);
            Ok(())
        }
        .await;

        // Mark sync as completed or failed
        match &result {
            Ok(_) => {
                self.sdk_client.complete(&sync_run_id, 1, 1, None).await?;
            }
            Err(e) => {
                self.sdk_client.fail(&sync_run_id, &e.to_string()).await?;
            }
        }

        result
    }

    pub async fn delete_page(
        &self,
        source_id: &str,
        sync_run_id: &str,
        space_key: &str,
        page_id: &str,
    ) -> Result<()> {
        info!("Deleting Confluence page: {}", page_id);

        let document_id = format!("confluence_page_{}_{}", space_key, page_id);
        let event = ConnectorEvent::DocumentDeleted {
            sync_run_id: sync_run_id.to_string(),
            source_id: source_id.to_string(),
            document_id,
        };

        self.sdk_client
            .emit_event(sync_run_id, source_id, event)
            .await?;
        info!("Successfully queued deletion for page: {}", page_id);
        Ok(())
    }
}
