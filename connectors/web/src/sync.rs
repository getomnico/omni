use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use dashmap::DashMap;
use redis::Client as RedisClient;
use spider::client::StatusCode;
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{mpsc, Mutex};
use tracing::{debug, error, info};

use crate::config::WebSourceConfig;
use crate::models::{PageSyncState, SyncRequest, WebPage};
use crate::sdk_client::SdkClient;

/// Result of a crawl operation
pub struct CrawlResult {
    pub pages_crawled: usize,
}

/// Trait for abstracting web page crawling
#[async_trait]
pub trait PageSource: Send + Sync {
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
        use redis::AsyncCommands;
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
        use redis::AsyncCommands;
        let mut conn = self.redis_client.get_multiplexed_async_connection().await?;
        let key = self.get_url_sync_key(source_id, url);
        let json_str = serde_json::to_string(state)?;

        let _: () = conn.set_ex(&key, json_str, 90 * 24 * 60 * 60).await?;
        Ok(())
    }

    pub async fn add_url_to_set(&self, source_id: &str, url: &str) -> Result<()> {
        use redis::AsyncCommands;
        let mut conn = self.redis_client.get_multiplexed_async_connection().await?;
        let key = self.get_urls_set_key(source_id);
        let url_hash = format!("{:x}", md5::compute(url));

        let _: () = conn.sadd(&key, url_hash).await?;
        let _: () = conn.expire(&key, 90 * 24 * 60 * 60).await?;
        Ok(())
    }

    pub async fn get_all_synced_urls(&self, source_id: &str) -> Result<HashSet<String>> {
        use redis::AsyncCommands;
        let mut conn = self.redis_client.get_multiplexed_async_connection().await?;
        let key = self.get_urls_set_key(source_id);

        let url_hashes: HashSet<String> = conn.smembers(&key).await?;
        Ok(url_hashes)
    }

    pub async fn remove_url_from_set(&self, source_id: &str, url_hash: &str) -> Result<()> {
        use redis::AsyncCommands;
        let mut conn = self.redis_client.get_multiplexed_async_connection().await?;
        let key = self.get_urls_set_key(source_id);

        let _: () = conn.srem(&key, url_hash).await?;
        Ok(())
    }
}

/// Tracks active syncs and their cancellation status
struct ActiveSync {
    cancelled: AtomicBool,
}

pub struct SyncManager {
    redis_client: RedisClient,
    sdk_client: SdkClient,
    page_source: Arc<dyn PageSource>,
    active_syncs: DashMap<String, Arc<ActiveSync>>,
}

impl SyncManager {
    pub fn new(redis_client: RedisClient, sdk_client: SdkClient) -> Self {
        Self::with_page_source(redis_client, sdk_client, Arc::new(SpiderPageSource))
    }

    pub fn with_page_source(
        redis_client: RedisClient,
        sdk_client: SdkClient,
        page_source: Arc<dyn PageSource>,
    ) -> Self {
        Self {
            redis_client,
            sdk_client,
            page_source,
            active_syncs: DashMap::new(),
        }
    }

    /// Execute a sync based on the request from connector-manager
    pub async fn sync_source(&self, request: SyncRequest) -> Result<()> {
        let sync_run_id = &request.sync_run_id;
        let source_id = &request.source.id;

        info!(
            "Starting sync for source: {} (sync_run_id: {})",
            source_id, sync_run_id
        );

        // Register this sync for cancellation support
        let active_sync = Arc::new(ActiveSync {
            cancelled: AtomicBool::new(false),
        });
        self.active_syncs
            .insert(sync_run_id.clone(), active_sync.clone());

        // Parse config from request
        let config = WebSourceConfig::from_json(&request.source.config)
            .context("Failed to parse web source config")?;

        let sync_state = SyncState::new(self.redis_client.clone());
        let previous_urls = sync_state.get_all_synced_urls(source_id).await?;
        let current_urls: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));

        let pages_processed: Arc<Mutex<usize>> = Arc::new(Mutex::new(0));
        let pages_updated: Arc<Mutex<usize>> = Arc::new(Mutex::new(0));

        // Create channel for receiving pages from the crawler
        let (tx, mut rx) = mpsc::channel::<WebPage>(32);

        // Spawn page processor
        let processor_handle = {
            let source_id = source_id.clone();
            let sync_run_id = sync_run_id.clone();
            let current_urls = current_urls.clone();
            let pages_processed = pages_processed.clone();
            let pages_updated = pages_updated.clone();
            let sync_state = sync_state.clone();
            let sdk_client = self.sdk_client.clone();
            let active_sync = active_sync.clone();

            tokio::spawn(async move {
                while let Some(web_page) = rx.recv().await {
                    // Check for cancellation
                    if active_sync.cancelled.load(Ordering::SeqCst) {
                        info!("Sync {} cancelled, stopping processor", sync_run_id);
                        break;
                    }

                    let page_url = web_page.url.clone();
                    debug!("Processing page: {}", page_url);

                    if let Err(e) = Self::process_web_page(
                        &web_page,
                        &sync_run_id,
                        &source_id,
                        &sync_state,
                        &current_urls,
                        &pages_processed,
                        &pages_updated,
                        &sdk_client,
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
        let crawl_result = self.page_source.crawl(&config, tx).await;

        // Wait for processor to finish
        processor_handle
            .await
            .with_context(|| "Failed while waiting for page processor to complete")?;

        // Check if cancelled
        if active_sync.cancelled.load(Ordering::SeqCst) {
            // Manager will handle updating status when cancel was requested
            self.active_syncs.remove(sync_run_id);
            return Ok(());
        }

        // Handle crawl errors
        if let Err(e) = crawl_result {
            if let Err(fail_err) = self.sdk_client.fail(sync_run_id, &e.to_string()).await {
                error!("Failed to report sync failure: {}", fail_err);
            }
            self.active_syncs.remove(sync_run_id);
            return Err(e);
        }

        debug!("Collecting final processed and updated document counts");
        let final_processed = *pages_processed.lock().await;
        let final_updated = *pages_updated.lock().await;

        // Handle deleted pages
        debug!("Collecting all URLs");
        let current_url_hashes = current_urls.lock().await;
        let deleted_urls: Vec<String> = previous_urls
            .difference(&*current_url_hashes)
            .cloned()
            .collect();

        info!(
            "Detected {} deleted pages for source {}",
            deleted_urls.len(),
            source_id
        );

        for url_hash in &deleted_urls {
            if let Err(e) = self
                .publish_deletion_event(sync_run_id, source_id, url_hash)
                .await
            {
                error!("Failed to publish deletion event: {}", e);
            }

            if let Err(e) = sync_state.remove_url_from_set(source_id, url_hash).await {
                error!("Failed to remove URL from set: {}", e);
            }
        }

        info!(
            "Completed sync for source {}: {} pages scanned, {} updated, {} deleted",
            source_id,
            final_processed,
            final_updated,
            deleted_urls.len()
        );

        // Mark sync as completed via SDK
        if let Err(e) = self
            .sdk_client
            .complete(
                sync_run_id,
                final_processed as i32,
                final_updated as i32,
                None,
            )
            .await
        {
            error!("Failed to mark sync as completed: {}", e);
        }

        self.active_syncs.remove(sync_run_id);
        Ok(())
    }

    /// Cancel a running sync
    pub fn cancel_sync(&self, sync_run_id: &str) -> bool {
        if let Some(active_sync) = self.active_syncs.get(sync_run_id) {
            active_sync.cancelled.store(true, Ordering::SeqCst);
            true
        } else {
            false
        }
    }

    async fn process_web_page(
        web_page: &WebPage,
        sync_run_id: &str,
        source_id: &str,
        sync_state: &SyncState,
        current_urls: &Arc<Mutex<HashSet<String>>>,
        pages_processed: &Arc<Mutex<usize>>,
        pages_updated: &Arc<Mutex<usize>>,
        sdk_client: &SdkClient,
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
            // Store content via SDK
            let content_id = sdk_client
                .store_content(sync_run_id, &web_page.content)
                .await
                .context("Failed to store page content")?;

            let event = web_page.to_connector_event(
                sync_run_id.to_string(),
                source_id.to_string(),
                content_id,
            );

            // Emit event via SDK
            sdk_client
                .emit_event(sync_run_id, source_id, event)
                .await
                .context("Failed to emit event")?;

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

        // Update activity and scanned count via SDK
        sdk_client
            .increment_scanned(sync_run_id)
            .await
            .context("Failed to update sync activity")?;

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

        self.sdk_client
            .emit_event(sync_run_id, source_id, event)
            .await?;
        Ok(())
    }
}
