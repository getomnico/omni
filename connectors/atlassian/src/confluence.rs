use anyhow::{anyhow, Result};
use futures::stream::StreamExt;
use redis::Client as RedisClient;
use shared::db::repositories::SyncRunRepository;
use shared::models::{ConnectorEvent, SyncRun, SyncType};
use shared::queue::EventQueue;
use shared::ObjectStorage;
use std::sync::Arc;
use tracing::{debug, error, info, warn};

use crate::auth::AtlassianCredentials;
use crate::client::AtlassianClient;
use crate::models::{ConfluencePage, ConfluencePageStatus, ConfluenceSpace};
use crate::sync::SyncState;

pub struct ConfluenceProcessor {
    client: AtlassianClient,
    event_queue: EventQueue,
    content_storage: Arc<dyn ObjectStorage>,
    sync_run_repo: SyncRunRepository,
    sync_state: SyncState,
}

impl ConfluenceProcessor {
    pub fn new(
        event_queue: EventQueue,
        content_storage: Arc<dyn ObjectStorage>,
        sync_run_repo: SyncRunRepository,
        redis_client: RedisClient,
    ) -> Self {
        Self {
            client: AtlassianClient::new(),
            event_queue,
            content_storage,
            sync_run_repo,
            sync_state: SyncState::new(redis_client),
        }
    }

    fn get_storage_prefix(sync_run: &SyncRun) -> String {
        format!(
            "{}/{}",
            sync_run
                .created_at
                .format(&time::format_description::well_known::Iso8601::DATE)
                .unwrap_or_else(|_| "unknown-date".to_string()),
            sync_run.id
        )
    }

    pub async fn sync_all_spaces(
        &mut self,
        creds: &AtlassianCredentials,
        source_id: &str,
    ) -> Result<u32> {
        info!("Starting Confluence spaces sync for source: {}", source_id);

        let sync_run = self.sync_run_repo.create(source_id, SyncType::Full).await?;
        let storage_prefix = Self::get_storage_prefix(&sync_run);

        let result: Result<u32> = async {
            let spaces = self.get_accessible_spaces(creds).await?;
            let mut total_pages_processed = 0;

            for space in spaces {
                info!(
                    "Syncing Confluence space: {} [key={}, id={}]",
                    space.name, space.key, space.id
                );

                match self
                    .sync_space_pages(creds, source_id, &sync_run.id, &space.id, &storage_prefix)
                    .await
                {
                    Ok(pages_count) => {
                        total_pages_processed += pages_count;
                        info!("Synced {} pages from space: {}", pages_count, space.id);
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
        .await;

        match &result {
            Ok(pages_processed) => {
                self.sync_run_repo
                    .mark_completed(
                        &sync_run.id,
                        *pages_processed as i32,
                        *pages_processed as i32,
                        *pages_processed as i32,
                    )
                    .await?;
            }
            Err(e) => {
                self.sync_run_repo
                    .mark_failed(&sync_run.id, &e.to_string())
                    .await?;
            }
        }

        result
    }

    async fn sync_space_pages(
        &mut self,
        creds: &AtlassianCredentials,
        source_id: &str,
        sync_run_id: &str,
        space_id: &str,
        storage_prefix: &str,
    ) -> Result<u32> {
        let mut total_pages = 0;
        let mut pages_batch = Vec::with_capacity(100);

        info!("Fetching pages for Confluence space {}", space_id);
        let mut pages_stream = self.client.get_confluence_pages(creds, space_id);

        while let Some(page_result) = pages_stream.next().await {
            let page = page_result?;
            pages_batch.push(page);

            if pages_batch.len() >= 100 {
                let events = self
                    .process_pages(
                        pages_batch,
                        source_id,
                        sync_run_id,
                        &creds.base_url,
                        storage_prefix,
                    )
                    .await?;
                total_pages += events.len() as u32;
                self.queue_events(events).await?;
                pages_batch = Vec::with_capacity(100);
            }
        }

        if !pages_batch.is_empty() {
            let events = self
                .process_pages(
                    pages_batch,
                    source_id,
                    sync_run_id,
                    &creds.base_url,
                    storage_prefix,
                )
                .await?;
            total_pages += events.len() as u32;
            self.queue_events(events).await?;
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
        storage_prefix: &str,
    ) -> Result<Vec<ConnectorEvent>> {
        let mut events = Vec::new();

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

            // Store content in storage
            let content_id = match self
                .content_storage
                .store_text(&content, Some(storage_prefix))
                .await
            {
                Ok(oid) => oid,
                Err(e) => {
                    error!(
                        "Failed to store content in storage for Confluence page {}: {}",
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
            events.push(event);

            // Update sync state
            if let Err(e) = self
                .sync_state
                .set_confluence_page_version(source_id, &page.space_id, &page.id, current_version)
                .await
            {
                warn!("Failed to update sync state for page {}: {}", page.id, e);
            }
        }

        Ok(events)
    }

    async fn queue_events(&self, events: Vec<ConnectorEvent>) -> Result<()> {
        for event in events {
            if let Err(e) = self.event_queue.enqueue(event.source_id(), &event).await {
                error!("Failed to queue Confluence event: {}", e);
                // Continue processing other events
            }
        }
        Ok(())
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

        let sync_run = self
            .sync_run_repo
            .create(source_id, SyncType::Incremental)
            .await?;
        let storage_prefix = Self::get_storage_prefix(&sync_run);

        let content_id = self
            .content_storage
            .store_text(&content, Some(&storage_prefix))
            .await
            .map_err(|e| {
                anyhow!(
                    "Failed to store content in storage for Confluence page {}: {}",
                    page.title,
                    e
                )
            })?;

        let event = page.to_connector_event(
            sync_run.id,
            source_id.to_string(),
            &creds.base_url,
            content_id,
        );
        self.event_queue.enqueue(source_id, &event).await?;

        info!("Successfully queued page: {}", page.title);
        Ok(())
    }

    pub async fn delete_page(&self, source_id: &str, space_key: &str, page_id: &str) -> Result<()> {
        info!("Deleting Confluence page: {}", page_id);

        let document_id = format!("confluence_page_{}_{}", space_key, page_id);
        // TODO: Add proper sync_run_id when sync runs are implemented for Atlassian
        let placeholder_sync_run_id = shared::utils::generate_ulid();
        let event = shared::models::ConnectorEvent::DocumentDeleted {
            sync_run_id: placeholder_sync_run_id,
            source_id: source_id.to_string(),
            document_id,
        };

        self.event_queue.enqueue(source_id, &event).await?;
        info!("Successfully queued deletion for page: {}", page_id);
        Ok(())
    }
}
