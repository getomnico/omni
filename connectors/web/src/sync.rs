use anyhow::{Context, Result};
use async_trait::async_trait;
use omni_connector_sdk::{SdkClient, SyncContext};
use spider::client::StatusCode;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{mpsc, Mutex};
use tracing::{debug, info};

use crate::config::WebSourceConfig;
use crate::models::{PageSyncState, WebConnectorState, WebPage};

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

        let mut rx = website.subscribe(32);

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

pub struct SyncManager {
    sdk_client: SdkClient,
    page_source: Arc<dyn PageSource>,
}

impl SyncManager {
    pub fn new(sdk_client: SdkClient) -> Self {
        Self::with_page_source(sdk_client, Arc::new(SpiderPageSource))
    }

    pub fn with_page_source(sdk_client: SdkClient, page_source: Arc<dyn PageSource>) -> Self {
        Self {
            sdk_client,
            page_source,
        }
    }

    pub fn sdk_client(&self) -> &SdkClient {
        &self.sdk_client
    }

    pub async fn run_sync(
        &self,
        config: WebSourceConfig,
        prior_state: Option<WebConnectorState>,
        ctx: SyncContext,
    ) -> Result<()> {
        let sync_run_id = ctx.sync_run_id().to_string();
        let source_id = ctx.source_id().to_string();

        // Hydrate prior state (empty on first sync).
        let prior = prior_state.unwrap_or_default();
        let previous_doc_ids: HashSet<String> = prior.pages.keys().cloned().collect();

        // In-memory state accumulated during this sync run. On completion it
        // replaces `prior` and is persisted via `ctx.save_connector_state`.
        let state: Arc<Mutex<HashMap<String, PageSyncState>>> =
            Arc::new(Mutex::new(prior.pages.clone()));
        let current_doc_ids: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));

        let pages_processed: Arc<Mutex<usize>> = Arc::new(Mutex::new(0));
        let pages_updated: Arc<Mutex<usize>> = Arc::new(Mutex::new(0));

        let (tx, mut rx) = mpsc::channel::<WebPage>(32);

        let processor_handle = {
            let source_id = source_id.clone();
            let sync_run_id = sync_run_id.clone();
            let current_doc_ids = current_doc_ids.clone();
            let state = state.clone();
            let pages_processed = pages_processed.clone();
            let pages_updated = pages_updated.clone();
            let ctx = ctx.clone();

            tokio::spawn(async move {
                while let Some(web_page) = rx.recv().await {
                    if ctx.is_cancelled() {
                        info!("Sync {} cancelled, stopping processor", sync_run_id);
                        break;
                    }

                    let page_url = web_page.url.clone();
                    debug!("Processing page: {}", page_url);

                    if let Err(e) = Self::process_web_page(
                        &web_page,
                        &sync_run_id,
                        &source_id,
                        &state,
                        &current_doc_ids,
                        &pages_processed,
                        &pages_updated,
                        &ctx,
                    )
                    .await
                    {
                        tracing::error!("Failed to process page {}: {}", page_url, e);
                    }
                }
            })
        };

        info!("Setting up crawl for url {}", config.root_url);
        let crawl_result = self.page_source.crawl(&config, tx).await;

        processor_handle
            .await
            .with_context(|| "Failed while waiting for page processor to complete")?;

        if ctx.is_cancelled() {
            return Ok(());
        }

        crawl_result?;

        let final_processed = *pages_processed.lock().await;
        let final_updated = *pages_updated.lock().await;

        // Deletion detection — any doc_id present in prior state but not seen
        // in the current crawl is treated as deleted. Emit the event and drop
        // it from the state so the next sync doesn't re-emit.
        let current = current_doc_ids.lock().await.clone();
        let deleted_doc_ids: Vec<String> = previous_doc_ids.difference(&current).cloned().collect();
        info!(
            "Detected {} deleted pages for source {}",
            deleted_doc_ids.len(),
            source_id
        );

        {
            let mut state_guard = state.lock().await;
            for doc_id in &deleted_doc_ids {
                if let Err(e) = self
                    .publish_deletion_event(&sync_run_id, &source_id, doc_id)
                    .await
                {
                    tracing::error!("Failed to publish deletion event: {}", e);
                }
                state_guard.remove(doc_id);
            }
        }

        info!(
            "Completed sync for source {}: {} pages scanned, {} updated, {} deleted",
            source_id,
            final_processed,
            final_updated,
            deleted_doc_ids.len()
        );

        let new_state = WebConnectorState {
            pages: state.lock().await.clone(),
            last_sync_completed_at: Some(chrono::Utc::now().to_rfc3339()),
        };
        ctx.save_connector_state(serde_json::to_value(&new_state)?)
            .await?;
        ctx.complete(final_processed as i32, final_updated as i32, None)
            .await?;
        Ok(())
    }

    async fn process_web_page(
        web_page: &WebPage,
        sync_run_id: &str,
        source_id: &str,
        state: &Arc<Mutex<HashMap<String, PageSyncState>>>,
        current_doc_ids: &Arc<Mutex<HashSet<String>>>,
        pages_processed: &Arc<Mutex<usize>>,
        pages_updated: &Arc<Mutex<usize>>,
        ctx: &SyncContext,
    ) -> Result<()> {
        let url = &web_page.url;
        let doc_id = WebPage::url_to_document_id(url);

        current_doc_ids.lock().await.insert(doc_id.clone());

        let should_index = match state.lock().await.get(&doc_id) {
            Some(prior) => {
                if prior.has_changed(web_page) {
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
            let content_id = ctx
                .extract_and_store_content(web_page.raw_html.as_bytes().to_vec(), "text/html", None)
                .await
                .context("Failed to extract and store page content")?;

            let event = web_page.to_connector_event(
                sync_run_id.to_string(),
                source_id.to_string(),
                content_id,
            );

            ctx.emit_event(event)
                .await
                .context("Failed to emit event")?;

            let new_entry = PageSyncState::new(web_page);
            state.lock().await.insert(doc_id, new_entry);

            *pages_updated.lock().await += 1;
        }

        *pages_processed.lock().await += 1;

        ctx.increment_scanned(1)
            .await
            .context("Failed to update sync activity")?;

        Ok(())
    }

    async fn publish_deletion_event(
        &self,
        sync_run_id: &str,
        source_id: &str,
        document_id: &str,
    ) -> Result<()> {
        let event = omni_connector_sdk::ConnectorEvent::DocumentDeleted {
            sync_run_id: sync_run_id.to_string(),
            source_id: source_id.to_string(),
            document_id: document_id.to_string(),
        };

        self.sdk_client
            .emit_event(sync_run_id, source_id, event)
            .await?;
        Ok(())
    }
}
