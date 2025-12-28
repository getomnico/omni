use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use redis::{AsyncCommands, Client as RedisClient};
use shared::db::repositories::{SourceRepository, SyncRunRepository};
use shared::models::{Source, SourceType, SyncRun, SyncType};
use shared::queue::EventQueue;
use shared::{ObjectStorage, Repository};
use spider::client::StatusCode;
use sqlx::PgPool;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;
use time;
use tokio::sync::{mpsc, Mutex};
use tracing::{debug, error, info};

use crate::config::WebSourceConfig;
use crate::models::{PageSyncState, WebPage};

/// Result of a crawl operation
pub struct CrawlResult {
    pub pages_crawled: usize,
}

/// Trait for abstracting web page crawling
#[async_trait]
pub trait PageSource: Send + Sync {
    /// Crawl pages and send them through the provided channel
    async fn crawl(
        &self,
        config: &WebSourceConfig,
        tx: mpsc::Sender<WebPage>,
    ) -> Result<CrawlResult>;
}

/// Real implementation using spider library
pub struct SpiderPageSource;

#[async_trait]
impl PageSource for SpiderPageSource {
    async fn crawl(
        &self,
        config: &WebSourceConfig,
        tx: mpsc::Sender<WebPage>,
    ) -> Result<CrawlResult> {
        let mut website = config.build_spider_website()?;

        let mut rx = website.subscribe(32).ok_or(anyhow!(
            "Failed to subscribe to website crawl events for root url: {}",
            config.root_url
        ))?;

        let processor_handle = tokio::spawn(async move {
            while let Ok(page) = rx.recv().await {
                if page.status_code != StatusCode::OK {
                    continue;
                }

                if let Ok(web_page) = WebPage::from_spider_page(&page) {
                    if tx.send(web_page).await.is_err() {
                        break;
                    }
                }
            }
        });

        info!("Starting crawl of {}", config.root_url);
        let crawl_start = Instant::now();
        website.crawl().await;

        let crawl_duration = crawl_start.elapsed();
        website.unsubscribe();
        processor_handle.await?;

        let links = website.get_links();
        info!(
            "Crawled {} pages from {} in {:?}",
            links.len(),
            config.root_url,
            crawl_duration
        );

        Ok(CrawlResult {
            pages_crawled: links.len(),
        })
    }
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
}

pub struct SyncManager {
    redis_client: RedisClient,
    event_queue: EventQueue,
    content_storage: Arc<dyn ObjectStorage>,
    sync_run_repo: SyncRunRepository,
    source_repo: SourceRepository,
    page_source: Arc<dyn PageSource>,
}

impl SyncManager {
    pub async fn new(pool: PgPool, redis_client: RedisClient) -> Result<Self> {
        Self::with_page_source(pool, redis_client, Arc::new(SpiderPageSource)).await
    }

    pub async fn with_page_source(
        pool: PgPool,
        redis_client: RedisClient,
        page_source: Arc<dyn PageSource>,
    ) -> Result<Self> {
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
            page_source,
        })
    }

    /// Create a SyncManager for testing with explicit dependencies
    pub fn new_for_testing(
        redis_client: RedisClient,
        event_queue: EventQueue,
        content_storage: Arc<dyn ObjectStorage>,
        sync_run_repo: SyncRunRepository,
        source_repo: SourceRepository,
        page_source: Arc<dyn PageSource>,
    ) -> Self {
        Self {
            redis_client,
            event_queue,
            content_storage,
            sync_run_repo,
            source_repo,
            page_source,
        }
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

        let sync_state = SyncState::new(self.redis_client.clone());
        let previous_urls = sync_state.get_all_synced_urls(&source.id).await?;
        let current_urls: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));

        let pages_processed: Arc<Mutex<usize>> = Arc::new(Mutex::new(0));
        let pages_updated: Arc<Mutex<usize>> = Arc::new(Mutex::new(0));

        // Create channel for receiving pages from the crawler
        let (tx, mut rx) = mpsc::channel::<WebPage>(32);

        // Spawn page processor
        let processor_handle = {
            let source_id = source.id.clone();
            let current_urls = current_urls.clone();
            let pages_processed = pages_processed.clone();
            let pages_updated = pages_updated.clone();
            let sync_state = sync_state.clone();
            let sync_run = sync_run.clone();
            let event_queue = self.event_queue.clone();
            let content_storage = self.content_storage.clone();
            let sync_run_repo = self.sync_run_repo.clone();

            tokio::spawn(async move {
                while let Some(web_page) = rx.recv().await {
                    let page_url = web_page.url.clone();
                    debug!("Processing page: {}", page_url);

                    if let Err(e) = Self::process_web_page(
                        &web_page,
                        &sync_run,
                        &source_id,
                        &sync_state,
                        &current_urls,
                        &pages_processed,
                        &pages_updated,
                        &event_queue,
                        &content_storage,
                        &sync_run_repo,
                    )
                    .await
                    {
                        error!("Failed to process page {}: {}", page_url, e);
                    }
                }
            })
        };

        // Start crawling
        info!("Setting up crawl for url {}", config.root_url);
        self.page_source.crawl(&config, tx).await?;

        // Wait for processor to finish
        processor_handle
            .await
            .with_context(|| "Failed while waiting for page processor to complete")?;

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

    async fn process_web_page(
        web_page: &WebPage,
        sync_run: &SyncRun,
        source_id: &str,
        sync_state: &SyncState,
        current_urls: &Arc<Mutex<HashSet<String>>>,
        pages_processed: &Arc<Mutex<usize>>,
        pages_updated: &Arc<Mutex<usize>>,
        event_queue: &EventQueue,
        content_storage: &Arc<dyn ObjectStorage>,
        sync_run_repo: &SyncRunRepository,
    ) -> Result<()> {
        let url = &web_page.url;
        let url_hash = format!("{:x}", md5::compute(url));

        {
            let mut urls = current_urls.lock().await;
            urls.insert(url_hash.clone());
        }

        let should_index = match sync_state.get_page_sync_state(source_id, url).await? {
            Some(old_state) => {
                if old_state.has_changed(web_page) {
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
            let content_id = content_storage
                .store_text(&web_page.content, Some(&storage_prefix))
                .await
                .context("Failed to store page content")?;

            let event = web_page.to_connector_event(
                sync_run.id.to_string(),
                source_id.to_string(),
                content_id,
            );

            event_queue
                .enqueue(source_id, &event)
                .await
                .context("Failed to enqueue event")?;

            let new_state = PageSyncState::new(web_page);
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
        sync_run_repo.increment_scanned(&sync_run.id, 1).await?;

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
