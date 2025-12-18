use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use redis::{AsyncCommands, Client as RedisClient};
use shared::db::repositories::{SourceRepository, SyncRunRepository};
use shared::models::{Source, SourceType, SyncRun, SyncType};
use shared::queue::EventQueue;
use shared::{ObjectStorage, Repository};
use spider::client::StatusCode;
use spider::page::Page;
use sqlx::PgPool;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;
use time;
use tokio::sync::Mutex;
use tracing::{debug, error, info, warn};

use crate::config::WebSourceConfig;
use crate::models::{PageSyncState, WebPage};

#[derive(Clone)]
pub struct SyncManager {
    redis_client: RedisClient,
    event_queue: EventQueue,
    content_storage: Arc<dyn ObjectStorage>,
    sync_run_repo: SyncRunRepository,
    source_repo: SourceRepository,
}

#[derive(Clone)]
pub struct SyncState {
    redis_client: RedisClient,
}

impl SyncState {
    pub fn new(redis_client: RedisClient) -> Self {
        Self { redis_client }
    }

    fn get_url_sync_key(&self, source_id: &str, url: &str) -> String {
        let url_hash = format!("{:x}", md5::compute(url));
        format!("web:sync:{}:{}", source_id, url_hash)
    }

    fn get_urls_set_key(&self, source_id: &str) -> String {
        format!("web:urls:{}", source_id)
    }

    pub async fn get_page_sync_state(
        &self,
        source_id: &str,
        url: &str,
    ) -> Result<Option<PageSyncState>> {
        let mut conn = self.redis_client.get_multiplexed_async_connection().await?;
        let key = self.get_url_sync_key(source_id, url);

        let result: Option<String> = conn.get(&key).await?;
        match result {
            Some(json_str) => {
                let state: PageSyncState = serde_json::from_str(&json_str)
                    .context("Failed to deserialize page sync state")?;
                Ok(Some(state))
            }
            None => Ok(None),
        }
    }

    pub async fn set_page_sync_state(
        &self,
        source_id: &str,
        url: &str,
        state: &PageSyncState,
    ) -> Result<()> {
        let mut conn = self.redis_client.get_multiplexed_async_connection().await?;
        let key = self.get_url_sync_key(source_id, url);
        let json_str = serde_json::to_string(state)?;

        let _: () = conn.set_ex(&key, json_str, 90 * 24 * 60 * 60).await?; // 90 days expiry
        Ok(())
    }

    pub async fn add_url_to_set(&self, source_id: &str, url: &str) -> Result<()> {
        let mut conn = self.redis_client.get_multiplexed_async_connection().await?;
        let key = self.get_urls_set_key(source_id);
        let url_hash = format!("{:x}", md5::compute(url));

        let _: () = conn.sadd(&key, url_hash).await?;
        let _: () = conn.expire(&key, 90 * 24 * 60 * 60).await?; // 90 days expiry
        Ok(())
    }

    pub async fn get_all_synced_urls(&self, source_id: &str) -> Result<HashSet<String>> {
        let mut conn = self.redis_client.get_multiplexed_async_connection().await?;
        let key = self.get_urls_set_key(source_id);

        let url_hashes: HashSet<String> = conn.smembers(&key).await?;
        Ok(url_hashes)
    }

    pub async fn remove_url_from_set(&self, source_id: &str, url_hash: &str) -> Result<()> {
        let mut conn = self.redis_client.get_multiplexed_async_connection().await?;
        let key = self.get_urls_set_key(source_id);

        let _: () = conn.srem(&key, url_hash).await?;
        Ok(())
    }

    pub async fn delete_page_sync_state(&self, source_id: &str, url: &str) -> Result<()> {
        let mut conn = self.redis_client.get_multiplexed_async_connection().await?;
        let key = self.get_url_sync_key(source_id, url);

        let _: () = conn.del(&key).await?;
        Ok(())
    }
}

impl SyncManager {
    pub async fn new(pool: PgPool, redis_client: RedisClient) -> Result<Self> {
        let event_queue = EventQueue::new(pool.clone());
        let content_storage = shared::StorageFactory::from_env(pool.clone()).await?;
        let sync_run_repo = SyncRunRepository::new(&pool);
        let source_repo = SourceRepository::new(&pool);

        Ok(Self {
            redis_client,
            event_queue,
            content_storage,
            sync_run_repo,
            source_repo,
        })
    }

    pub async fn sync_all_sources(&self) -> Result<()> {
        let sources = self.get_active_sources().await?;

        info!("Found {} active Web sources", sources.len());

        for source in sources {
            if let Err(e) = self.sync_source(&source).await {
                error!("Failed to sync source {}: {}", source.id, e);
                let _ = self
                    .update_source_status(&source.id, "failed", None, Some(e.to_string()))
                    .await;
            }
        }

        Ok(())
    }

    pub async fn sync_source_by_id(&self, source_id: &str) -> Result<()> {
        let source = self.get_source_by_id(source_id).await?;
        self.sync_source(&source).await
    }

    async fn get_active_sources(&self) -> Result<Vec<Source>> {
        let sources = self
            .source_repo
            .find_active_by_types(vec![SourceType::Web])
            .await?;

        Ok(sources)
    }

    async fn get_source_by_id(&self, source_id: &str) -> Result<Source> {
        let source = self
            .source_repo
            .find_by_id(source_id.to_string())
            .await?
            .ok_or_else(|| anyhow::anyhow!("Source not found: {}", source_id))?;

        // Verify it's a web source
        if source.source_type != SourceType::Web {
            return Err(anyhow::anyhow!(
                "Source {} is not a web source (type: {:?})",
                source_id,
                source.source_type
            ));
        }

        Ok(source)
    }

    async fn sync_source(&self, source: &Source) -> Result<()> {
        info!(
            "Starting sync for web source: {} ({})",
            source.name, source.id
        );

        let config = WebSourceConfig::from_json(&source.config)
            .context("Failed to parse web source config")?;

        let sync_run = self
            .sync_run_repo
            .create(&source.id, SyncType::Full)
            .await?;

        let mut website = config.build_spider_website()?;

        let sync_state = SyncState::new(self.redis_client.clone());
        let previous_urls = sync_state.get_all_synced_urls(&source.id).await?;
        let current_urls: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));

        let pages_processed: Arc<Mutex<usize>> = Arc::new(Mutex::new(0));
        let pages_updated: Arc<Mutex<usize>> = Arc::new(Mutex::new(0));

        info!("Setting up subscription for url {}", config.root_url);
        let mut rx = website.subscribe(32).ok_or(anyhow!(
            "Failed to subscribe to website crawl events for root url: {}",
            config.root_url
        ))?;
        let processor_handle = tokio::spawn({
            let self_clone = self.clone();
            let source = source.clone();
            let current_urls = current_urls.clone();
            let pages_processed = pages_processed.clone();
            let pages_updated = pages_updated.clone();
            let sync_state = sync_state.clone();
            let sync_run = sync_run.clone();

            async move {
                while let Ok(page) = rx.recv().await {
                    let page_title = page.metadata.as_ref().and_then(|m| m.title.clone());
                    let page_url = page.get_url();
                    debug!(
                        "Received crawl event for page {:?} [url={}]",
                        page_title, page_url
                    );

                    if page.status_code != StatusCode::OK {
                        debug!("Page {:?} [url={}] status code is not 200 OK [status_code={}], skipping", page_title, page_url, page.status_code);
                        continue;
                    }

                    if let Err(e) = self_clone
                        .process_page(
                            &page,
                            &sync_run,
                            &source.id,
                            &sync_state,
                            &current_urls,
                            &pages_processed,
                            &pages_updated,
                        )
                        .await
                    {
                        error!("Failed to process page {}: {}", page_url, e)
                    }

                    debug!("Processed page {:?} [url={}]", page_title, page_url);
                }
            }
        });

        info!("Starting crawl of {}", config.root_url);
        let crawl_start = Instant::now();
        website.crawl().await;

        info!("Crawl complete, waiting for processing task to complete...");
        let crawl_duration = crawl_start.elapsed();
        website.unsubscribe();
        processor_handle
            .await
            .with_context(|| format!("Failed while waiting for tasks to complete"))?;

        let links = website.get_links();
        info!(
            "Crawled {} pages from {} in {:?}",
            links.len(),
            config.root_url,
            crawl_duration
        );

        debug!("Collecting final processed and updated document counts");
        let final_processed = *pages_processed.lock().await;
        let final_updated = *pages_updated.lock().await;

        debug!("Collecting all URLs");
        let current_url_hashes = current_urls.lock().await;
        let deleted_urls: Vec<String> = previous_urls
            .difference(&*current_url_hashes)
            .cloned()
            .collect();

        info!(
            "Detected {} deleted pages for source {}",
            deleted_urls.len(),
            source.id
        );

        for url_hash in &deleted_urls {
            if let Err(e) = self
                .publish_deletion_event(&sync_run.id, &source.id, url_hash)
                .await
            {
                error!("Failed to publish deletion event: {}", e);
            }

            if let Err(e) = sync_state.remove_url_from_set(&source.id, url_hash).await {
                error!("Failed to remove URL from set: {}", e);
            }
        }

        info!(
            "Completed sync for source {}: {} pages scanned, {} updated, {} deleted",
            source.id,
            final_processed,
            final_updated,
            deleted_urls.len()
        );

        self.sync_run_repo
            .mark_completed(&sync_run.id, final_processed as i32, final_updated as i32)
            .await?;

        self.update_source_status(&source.id, "completed", Some(Utc::now()), None)
            .await?;

        Ok(())
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

    async fn update_source_status(
        &self,
        source_id: &str,
        status: &str,
        last_sync_at: Option<DateTime<Utc>>,
        sync_error: Option<String>,
    ) -> Result<()> {
        self.source_repo
            .update_sync_status(source_id, status, last_sync_at, sync_error)
            .await?;

        Ok(())
    }

    async fn process_page(
        &self,
        page: &Page,
        sync_run: &SyncRun,
        source_id: &str,
        sync_state: &SyncState,
        current_urls: &Arc<Mutex<HashSet<String>>>,
        pages_processed: &Arc<Mutex<usize>>,
        pages_updated: &Arc<Mutex<usize>>,
    ) -> Result<()> {
        let web_page =
            WebPage::from_spider_page(page).context("Failed to extract content from page")?;

        let url = &web_page.url;
        let url_hash = format!("{:x}", md5::compute(url));

        {
            let mut urls = current_urls.lock().await;
            urls.insert(url_hash.clone());
        }

        let should_index = match sync_state.get_page_sync_state(source_id, url).await? {
            Some(old_state) => {
                if old_state.has_changed(&web_page) {
                    debug!("Page {} has changed, will update", url);
                    true
                } else {
                    debug!("Page {} unchanged, skipping", url);
                    false
                }
            }
            None => {
                debug!("New page {}, will index", url);
                true
            }
        };

        if should_index {
            let storage_prefix = Self::get_storage_prefix(sync_run);
            let content_id = self
                .content_storage
                .store_text(&web_page.content, Some(&storage_prefix))
                .await
                .context("Failed to store page content")?;

            let event = web_page.to_connector_event(
                sync_run.id.to_string(),
                source_id.to_string(),
                content_id,
            );

            self.event_queue
                .enqueue(source_id, &event)
                .await
                .context("Failed to enqueue event")?;

            let new_state = PageSyncState::new(&web_page);
            sync_state
                .set_page_sync_state(source_id, url, &new_state)
                .await?;

            sync_state.add_url_to_set(source_id, url).await?;

            let mut count = pages_updated.lock().await;
            *count += 1;
        }

        let mut count = pages_processed.lock().await;
        *count += 1;

        // Update scanned count
        self.sync_run_repo
            .increment_scanned(&sync_run.id, 1)
            .await?;

        Ok(())
    }

    async fn publish_deletion_event(
        &self,
        sync_run_id: &str,
        source_id: &str,
        url_hash: &str,
    ) -> Result<()> {
        let event = shared::models::ConnectorEvent::DocumentDeleted {
            sync_run_id: sync_run_id.to_string(),
            source_id: source_id.to_string(),
            document_id: url_hash.to_string(),
        };

        self.event_queue.enqueue(source_id, &event).await?;
        Ok(())
    }
}
