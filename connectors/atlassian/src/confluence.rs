use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use shared::models::ConnectorEvent;
use shared::queue::EventQueue;
use tracing::{debug, error, info, warn};

use crate::auth::AtlassianCredentials;
use crate::client::AtlassianClient;
use crate::models::{ConfluencePage, ConfluenceSearchResponse};

pub struct ConfluenceProcessor {
    client: AtlassianClient,
    event_queue: EventQueue,
}

impl ConfluenceProcessor {
    pub fn new(event_queue: EventQueue) -> Self {
        Self {
            client: AtlassianClient::new(),
            event_queue,
        }
    }

    pub async fn sync_all_spaces(
        &mut self,
        creds: &AtlassianCredentials,
        source_id: &str,
    ) -> Result<u32> {
        info!("Starting Confluence spaces sync for source: {}", source_id);

        let spaces = self.get_accessible_spaces(creds).await?;
        let mut total_pages_processed = 0;

        for space in spaces {
            let space_key = space
                .get("key")
                .and_then(|k| k.as_str())
                .ok_or_else(|| anyhow!("Space missing key"))?;

            let space_name = space
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or("Unknown Space");

            info!("Syncing Confluence space: {} ({})", space_name, space_key);

            match self.sync_space_pages(creds, source_id, space_key).await {
                Ok(pages_count) => {
                    total_pages_processed += pages_count;
                    info!("Synced {} pages from space: {}", pages_count, space_key);
                }
                Err(e) => {
                    error!("Failed to sync space {}: {}", space_key, e);
                    // Continue with other spaces
                }
            }
        }

        info!(
            "Completed Confluence sync. Total pages processed: {}",
            total_pages_processed
        );
        Ok(total_pages_processed)
    }

    pub async fn sync_pages_updated_since(
        &mut self,
        creds: &AtlassianCredentials,
        source_id: &str,
        since: DateTime<Utc>,
    ) -> Result<u32> {
        info!(
            "Starting incremental Confluence sync for source: {} since {}",
            source_id,
            since.format("%Y-%m-%d %H:%M:%S")
        );

        let since_str = since.format("%Y-%m-%d %H:%M").to_string();
        let mut total_pages = 0;
        let mut start = 0;
        const PAGE_SIZE: u32 = 50;

        loop {
            let response = self
                .client
                .get_confluence_pages_updated_since(creds, &since_str, PAGE_SIZE, start)
                .await?;

            if response.results.is_empty() {
                break;
            }

            let events = self.process_pages(response.results, source_id, &creds.base_url)?;
            self.queue_events(events).await?;

            total_pages += response.size as u32;
            start += PAGE_SIZE;

            debug!(
                "Processed {} pages, total so far: {}",
                response.size, total_pages
            );

            // Check if we've reached the end
            if response.size < PAGE_SIZE as i32 {
                break;
            }
        }

        info!(
            "Completed incremental Confluence sync. Pages processed: {}",
            total_pages
        );
        Ok(total_pages)
    }

    async fn sync_space_pages(
        &mut self,
        creds: &AtlassianCredentials,
        source_id: &str,
        space_key: &str,
    ) -> Result<u32> {
        let mut total_pages = 0;
        let mut start = 0;
        const PAGE_SIZE: u32 = 50;

        loop {
            let expand = vec![
                "body.storage",
                "space",
                "version",
                "ancestors",
                "_links.webui",
            ];

            let response = self
                .client
                .get_confluence_pages(creds, Some(space_key), PAGE_SIZE, start, &expand)
                .await?;

            if response.results.is_empty() {
                break;
            }

            let events = self.process_pages(response.results, source_id, &creds.base_url)?;
            self.queue_events(events).await?;

            total_pages += response.size as u32;
            start += PAGE_SIZE;

            debug!(
                "Processed {} pages from space {}, total: {}",
                response.size, space_key, total_pages
            );

            // Check if we've reached the end
            if response.size < PAGE_SIZE as i32 {
                break;
            }
        }

        Ok(total_pages)
    }

    async fn get_accessible_spaces(
        &mut self,
        creds: &AtlassianCredentials,
    ) -> Result<Vec<serde_json::Value>> {
        let mut all_spaces = Vec::new();
        let mut start = 0;
        const PAGE_SIZE: u32 = 50;

        loop {
            let response = self
                .client
                .get_confluence_spaces(creds, PAGE_SIZE, start)
                .await?;

            let spaces = response
                .get("results")
                .and_then(|r| r.as_array())
                .ok_or_else(|| anyhow!("Invalid spaces response format"))?;

            if spaces.is_empty() {
                break;
            }

            all_spaces.extend(spaces.iter().cloned());
            start += PAGE_SIZE;

            let size = response.get("size").and_then(|s| s.as_i64()).unwrap_or(0) as u32;

            if size < PAGE_SIZE {
                break;
            }
        }

        debug!("Found {} accessible Confluence spaces", all_spaces.len());
        Ok(all_spaces)
    }

    fn process_pages(
        &self,
        pages: Vec<ConfluencePage>,
        source_id: &str,
        base_url: &str,
    ) -> Result<Vec<ConnectorEvent>> {
        let mut events = Vec::new();

        for page in pages {
            // Skip non-current pages (drafts, trashed, etc.)
            if page.status != "current" {
                debug!("Skipping page {} with status: {}", page.id, page.status);
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
                page.space.name,
                content.len()
            );

            // TODO: Add proper sync_run_id when sync runs are implemented for Atlassian
            let placeholder_sync_run_id = shared::utils::generate_ulid();
            let event =
                page.to_connector_event(placeholder_sync_run_id, source_id.to_string(), base_url);
            events.push(event);
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

        if page.status != "current" {
            warn!(
                "Page {} is not current (status: {}), skipping",
                page_id, page.status
            );
            return Ok(());
        }

        let content = page.extract_plain_text();
        if content.trim().is_empty() {
            warn!("Page {} has no content, skipping", page_id);
            return Ok(());
        }

        // TODO: Add proper sync_run_id when sync runs are implemented for Atlassian
        let placeholder_sync_run_id = shared::utils::generate_ulid();
        let event = page.to_connector_event(
            placeholder_sync_run_id,
            source_id.to_string(),
            &creds.base_url,
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

    pub fn get_rate_limit_info(&self) -> String {
        let rate_limit = self.client.get_rate_limit_info();
        if let Some(remaining) = rate_limit.requests_remaining {
            format!("Requests remaining: {}", remaining)
        } else {
            "Rate limit info not available".to_string()
        }
    }
}
