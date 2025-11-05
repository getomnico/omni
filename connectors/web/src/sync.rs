use anyhow::{Context, Result};
use redis::{AsyncCommands, Client as RedisClient};
use shared::models::{Source, SourceType};
use shared::queue::EventQueue;
use shared::ObjectStorage;
use sqlx::PgPool;
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{debug, error, info, warn};

use crate::config::WebSourceConfig;
use crate::models::{PageSyncState, WebPage};

pub struct SyncManager {
    pool: PgPool,
    redis_client: RedisClient,
    event_queue: EventQueue,
    content_storage: Arc<dyn ObjectStorage>,
}

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

        Ok(Self {
            pool,
            redis_client,
            event_queue,
            content_storage,
        })
    }

    pub async fn sync_all_sources(&self) -> Result<()> {
        let sources = self.get_active_sources().await?;

        info!("Found {} active Web sources", sources.len());

        for source in sources {
            if let Err(e) = self.sync_source(&source).await {
                error!("Failed to sync source {}: {}", source.id, e);
                self.mark_sync_failed(&source.id, &e.to_string()).await?;
            }
        }

        Ok(())
    }

    pub async fn sync_source_by_id(&self, source_id: &str) -> Result<()> {
        let source = self.get_source_by_id(source_id).await?;
        self.sync_source(&source).await
    }

    async fn get_active_sources(&self) -> Result<Vec<Source>> {
        let sources = sqlx::query_as::<_, Source>(
            "SELECT * FROM sources WHERE source_type = $1 AND is_active = true",
        )
        .bind(SourceType::Web)
        .fetch_all(&self.pool)
        .await?;

        Ok(sources)
    }

    async fn get_source_by_id(&self, source_id: &str) -> Result<Source> {
        let source =
            sqlx::query_as::<_, Source>("SELECT * FROM sources WHERE id = $1 AND source_type = $2")
                .bind(source_id)
                .bind(SourceType::Web)
                .fetch_one(&self.pool)
                .await?;

        Ok(source)
    }

    async fn sync_source(&self, source: &Source) -> Result<()> {
        info!(
            "Starting sync for web source: {} ({})",
            source.name, source.id
        );

        let config = WebSourceConfig::from_json(&source.config)
            .context("Failed to parse web source config")?;

        let sync_run_id = self.create_sync_run(&source.id).await?;

        let mut website = config.build_spider_website()?;

        let sync_state = SyncState::new(self.redis_client.clone());
        let previous_urls = sync_state.get_all_synced_urls(&source.id).await?;
        let current_urls: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));

        let pages_processed: Arc<Mutex<usize>> = Arc::new(Mutex::new(0));
        let pages_updated: Arc<Mutex<usize>> = Arc::new(Mutex::new(0));

        info!("Starting crawl of {}", config.root_url);

        website.crawl().await;

        let links = website.get_links();
        info!("Crawled {} pages from {}", links.len(), config.root_url);

        for page in website.get_pages().unwrap_or(&vec![]) {
            let url = page.get_url();

            if let Err(e) = self
                .process_page(
                    page,
                    &sync_run_id,
                    &source.id,
                    &sync_state,
                    &current_urls,
                    &pages_processed,
                    &pages_updated,
                )
                .await
            {
                warn!("Failed to process page {}: {}", url, e);
            }
        }

        let final_processed = *pages_processed.lock().await;
        let final_updated = *pages_updated.lock().await;

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
                .publish_deletion_event(&sync_run_id, &source.id, url_hash)
                .await
            {
                error!("Failed to publish deletion event: {}", e);
            }

            if let Err(e) = sync_state.remove_url_from_set(&source.id, url_hash).await {
                error!("Failed to remove URL from set: {}", e);
            }
        }

        self.complete_sync_run(&sync_run_id, final_processed, final_updated)
            .await?;

        info!(
            "Completed sync for source {}: {} pages processed, {} updated, {} deleted",
            source.id,
            final_processed,
            final_updated,
            deleted_urls.len()
        );

        Ok(())
    }

    async fn process_page(
        &self,
        page: &spider::page::Page,
        sync_run_id: &str,
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
            let storage_prefix = format!("web/{}", source_id);
            let content_id = self
                .content_storage
                .store_text(&web_page.content, Some(&storage_prefix))
                .await
                .context("Failed to store page content")?;

            let event = web_page.to_connector_event(
                sync_run_id.to_string(),
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

    async fn create_sync_run(&self, source_id: &str) -> Result<String> {
        let sync_run_id = uuid::Uuid::new_v4().to_string();

        sqlx::query(
            "INSERT INTO sync_runs (id, source_id, status, started_at)
             VALUES ($1, $2, 'running', NOW())",
        )
        .bind(&sync_run_id)
        .bind(source_id)
        .execute(&self.pool)
        .await?;

        sqlx::query("NOTIFY sync_status_update")
            .execute(&self.pool)
            .await?;

        Ok(sync_run_id)
    }

    async fn complete_sync_run(
        &self,
        sync_run_id: &str,
        pages_processed: usize,
        pages_updated: usize,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE sync_runs
             SET status = 'completed',
                 completed_at = NOW(),
                 documents_processed = $2,
                 documents_updated = $3
             WHERE id = $1",
        )
        .bind(sync_run_id)
        .bind(pages_processed as i32)
        .bind(pages_updated as i32)
        .execute(&self.pool)
        .await?;

        sqlx::query("NOTIFY sync_status_update")
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    async fn mark_sync_failed(&self, source_id: &str, error_msg: &str) -> Result<()> {
        sqlx::query(
            "UPDATE sync_runs
             SET status = 'failed',
                 completed_at = NOW(),
                 error_message = $2
             WHERE source_id = $1
             AND status = 'running'
             ORDER BY started_at DESC
             LIMIT 1",
        )
        .bind(source_id)
        .bind(error_msg)
        .execute(&self.pool)
        .await?;

        sqlx::query("NOTIFY sync_status_update")
            .execute(&self.pool)
            .await?;

        Ok(())
    }
}
