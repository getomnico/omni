use crate::people_extractor;
use crate::AppState;
use anyhow::{Context, Result};
use shared::db::repositories::{
    DocumentRepository, EmbeddingRepository, GroupRepository, PersonRepository, SyncRunRepository,
};
use shared::embedding_queue::EmbeddingQueue;
use shared::models::{
    ConnectorEvent, ConnectorEventQueueItem, Document, DocumentAttributes, DocumentMetadata,
    DocumentPermissions, EventStatus, SyncType,
};
use shared::queue::EventQueue;
use shared::storage::gc::{ContentBlobGC, GCConfig};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, Semaphore};
use tokio::time::{interval, Duration, MissedTickBehavior};
use tracing::{debug, error, info, warn};

// Default poll interval for draining the queue. Overridable via INDEXER_POLL_INTERVAL_SECS.
// SDK-side buffering already shapes events into the right batch size per sync type,
// so the indexer just drains whatever's there on each tick.
const DEFAULT_POLL_INTERVAL_SECS: u64 = 60;

// Per-SyncType batching thresholds for the indexer.
//
// The indexer polls frequently but only writes when one of these thresholds is met.
// This lets small incremental trickles accumulate while full-sync bursts go through
// quickly (they are already well-shaped by the connector-side SDK buffer).
//
// All values are overridable via environment variables.
const DEFAULT_FULL_BATCH_SIZE: i64 = 1000;
const DEFAULT_FULL_BATCH_MAX_AGE_SECS: i64 = 300;
const DEFAULT_INCREMENTAL_BATCH_SIZE: i64 = 100;
const DEFAULT_INCREMENTAL_BATCH_MAX_AGE_SECS: i64 = 60;
const DEFAULT_REALTIME_BATCH_SIZE: i64 = 1;
const DEFAULT_REALTIME_BATCH_MAX_AGE_SECS: i64 = 0;
const DEFAULT_GLOBAL_BATCH_MAX_AGE_SECS: i64 = 300;

#[derive(Clone)]
struct BatchingConfig {
    full_batch_size: i64,
    full_max_age_secs: i64,
    incremental_batch_size: i64,
    incremental_max_age_secs: i64,
    realtime_batch_size: i64,
    realtime_max_age_secs: i64,
    global_max_age_secs: i64,
}

impl Default for BatchingConfig {
    fn default() -> Self {
        Self {
            full_batch_size: DEFAULT_FULL_BATCH_SIZE,
            full_max_age_secs: DEFAULT_FULL_BATCH_MAX_AGE_SECS,
            incremental_batch_size: DEFAULT_INCREMENTAL_BATCH_SIZE,
            incremental_max_age_secs: DEFAULT_INCREMENTAL_BATCH_MAX_AGE_SECS,
            realtime_batch_size: DEFAULT_REALTIME_BATCH_SIZE,
            realtime_max_age_secs: DEFAULT_REALTIME_BATCH_MAX_AGE_SECS,
            global_max_age_secs: DEFAULT_GLOBAL_BATCH_MAX_AGE_SECS,
        }
    }
}

impl BatchingConfig {
    fn from_env() -> Self {
        Self {
            full_batch_size: env_or("INDEXER_FULL_BATCH_SIZE", DEFAULT_FULL_BATCH_SIZE),
            full_max_age_secs: env_or(
                "INDEXER_FULL_BATCH_MAX_AGE_SECS",
                DEFAULT_FULL_BATCH_MAX_AGE_SECS,
            ),
            incremental_batch_size: env_or(
                "INDEXER_INCREMENTAL_BATCH_SIZE",
                DEFAULT_INCREMENTAL_BATCH_SIZE,
            ),
            incremental_max_age_secs: env_or(
                "INDEXER_INCREMENTAL_BATCH_MAX_AGE_SECS",
                DEFAULT_INCREMENTAL_BATCH_MAX_AGE_SECS,
            ),
            realtime_batch_size: env_or("INDEXER_REALTIME_BATCH_SIZE", DEFAULT_REALTIME_BATCH_SIZE),
            realtime_max_age_secs: env_or(
                "INDEXER_REALTIME_BATCH_MAX_AGE_SECS",
                DEFAULT_REALTIME_BATCH_MAX_AGE_SECS,
            ),
            global_max_age_secs: env_or(
                "INDEXER_GLOBAL_BATCH_MAX_AGE_SECS",
                DEFAULT_GLOBAL_BATCH_MAX_AGE_SECS,
            ),
        }
    }

    /// Returns the list of sync types whose pending events meet a threshold.
    fn ready_sync_types(
        &self,
        by_sync_type: &HashMap<SyncType, (i64, i64)>,
        _orphan_count: i64,
    ) -> Vec<(SyncType, String)> {
        let mut ready = Vec::new();

        for (sync_type, (count, oldest_age_secs)) in by_sync_type {
            let (size_threshold, age_threshold) = match sync_type {
                SyncType::Full => (self.full_batch_size, self.full_max_age_secs),
                SyncType::Incremental => {
                    (self.incremental_batch_size, self.incremental_max_age_secs)
                }
                SyncType::Realtime => (self.realtime_batch_size, self.realtime_max_age_secs),
            };

            if *count >= size_threshold {
                ready.push((
                    sync_type.clone(),
                    format!("{} count {} >= {}", sync_type, count, size_threshold),
                ));
            } else if *oldest_age_secs >= age_threshold {
                ready.push((
                    sync_type.clone(),
                    format!(
                        "{} age {}s >= {}s",
                        sync_type, oldest_age_secs, age_threshold
                    ),
                ));
            }
        }

        // Global safety net: never let events stall longer than this.
        let oldest_any = by_sync_type
            .values()
            .map(|(_, age)| *age)
            .chain(std::iter::once(0))
            .max()
            .unwrap_or(0);
        if oldest_any >= self.global_max_age_secs {
            for (sync_type, _) in by_sync_type {
                if !ready.iter().any(|(st, _)| st == sync_type) {
                    ready.push((
                        sync_type.clone(),
                        format!(
                            "global max age {}s >= {}s",
                            oldest_any, self.global_max_age_secs
                        ),
                    ));
                }
            }
        }

        ready
    }
}

/// Pending count and oldest age (seconds) for a sync type.
type PendingBySyncType = HashMap<SyncType, (i64, i64)>;

fn summarize_pending(summary: &shared::queue::QueueSummary) -> (PendingBySyncType, i64) {
    let mut by_sync_type = PendingBySyncType::new();
    let mut orphan_count = 0i64;

    for entry in &summary.entries {
        if entry.status != EventStatus::Pending {
            continue;
        }
        let oldest_age_secs = entry
            .oldest
            .map(|t| (chrono::Utc::now() - t).num_seconds())
            .unwrap_or(0);

        match &entry.sync_type {
            None => orphan_count = entry.count,
            Some(st) => {
                by_sync_type.insert(st.clone(), (entry.count, oldest_age_secs));
            }
        }
    }

    (by_sync_type, orphan_count)
}

fn env_or<T: std::str::FromStr>(key: &str, default: T) -> T {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

// Batch processing types
#[derive(Debug)]
struct GroupSyncEvent {
    source_id: String,
    group_email: String,
    group_name: Option<String>,
    member_emails: Vec<String>,
    event_ids: Vec<String>,
}

#[derive(Debug)]
struct EventBatch {
    sync_run_id: String,
    documents_upsert: Vec<(Document, Vec<String>)>, // (document, event_ids) — both creates and updates
    documents_deleted: Vec<(String, String, Vec<String>)>, // (source_id, document_id, event_ids)
    group_syncs: Vec<GroupSyncEvent>,
}

impl EventBatch {
    fn new(sync_run_id: String) -> Self {
        Self {
            sync_run_id,
            documents_upsert: Vec::new(),
            documents_deleted: Vec::new(),
            group_syncs: Vec::new(),
        }
    }

    fn is_empty(&self) -> bool {
        self.documents_upsert.is_empty()
            && self.documents_deleted.is_empty()
            && self.group_syncs.is_empty()
    }
}

#[derive(Debug)]
struct BatchProcessingResult {
    successful_event_ids: Vec<String>,
    successful_documents_count: usize,
    failed_events: Vec<(String, String)>, // (event_id, error_message)
}

impl BatchProcessingResult {
    fn new() -> Self {
        Self {
            successful_event_ids: Vec::new(),
            successful_documents_count: 0,
            failed_events: Vec::new(),
        }
    }
}

#[derive(Clone)]
pub struct QueueProcessor {
    pub state: AppState,
    pub event_queue: EventQueue,
    pub embedding_queue: EmbeddingQueue,
    pub sync_run_repo: SyncRunRepository,
    pub batch_size: i32,
    processing_mutex: Arc<Mutex<()>>,
    poll_interval: Duration,
    batching_config: BatchingConfig,
}

impl QueueProcessor {
    pub fn new(state: AppState) -> Self {
        let event_queue = EventQueue::new(state.db_pool.pool().clone());
        let embedding_queue = EmbeddingQueue::new(state.db_pool.pool().clone());
        let sync_run_repo = SyncRunRepository::new(state.db_pool.pool());
        let processing_mutex = Arc::new(Mutex::new(()));
        let batch_size = env_or("INDEXER_BATCH_SIZE", 2000);
        let poll_interval_secs = env_or("INDEXER_POLL_INTERVAL_SECS", DEFAULT_POLL_INTERVAL_SECS);
        Self {
            state,
            event_queue,
            embedding_queue,
            sync_run_repo,
            batch_size,
            processing_mutex,
            poll_interval: Duration::from_secs(poll_interval_secs),
            batching_config: BatchingConfig::from_env(),
        }
    }

    pub fn with_batch_size(mut self, batch_size: i32) -> Self {
        self.batch_size = batch_size;
        self
    }

    pub fn with_poll_interval(mut self, poll_interval: Duration) -> Self {
        self.poll_interval = poll_interval;
        self
    }

    #[allow(dead_code)]
    fn with_batching_config(mut self, config: BatchingConfig) -> Self {
        self.batching_config = config;
        self
    }

    pub async fn start(&self) -> Result<()> {
        info!(
            "Starting queue processor with batch size: {}",
            self.batch_size
        );

        // Recover any stale processing items from previous runs (5 minute timeout)
        match self.event_queue.recover_stale_processing_items(300).await {
            Ok(recovered) => {
                if recovered > 0 {
                    info!("Recovered {} stale processing items on startup", recovered);
                }
            }
            Err(e) => {
                error!("Failed to recover stale processing items on startup: {}", e);
            }
        }

        // Recover stale embedding queue items
        match self
            .embedding_queue
            .recover_stale_processing_items(300)
            .await
        {
            Ok(recovered) => {
                if recovered > 0 {
                    info!(
                        "Recovered {} stale embedding processing items on startup",
                        recovered
                    );
                }
            }
            Err(e) => {
                error!(
                    "Failed to recover stale embedding processing items on startup: {}",
                    e
                );
            }
        }

        let mut poll_interval = interval(self.poll_interval);
        poll_interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let mut heartbeat_interval = interval(Duration::from_secs(300));
        let mut retry_interval = interval(Duration::from_secs(300)); // 5 minutes
        let mut cleanup_interval = interval(Duration::from_secs(3600)); // 1 hour
        let mut recovery_interval = interval(Duration::from_secs(300)); // 5 minutes
        let mut gc_interval = interval(Duration::from_secs(3600 * 6)); // 6 hours

        // GC runs off the main select as its own task so a long sweep cannot stall
        // event processing. The semaphore bounds concurrent runs to 1; overlapping
        // ticks are skipped.
        let gc_semaphore = Arc::new(Semaphore::new(1));

        info!(
            "Queue processor poll interval: {:?}, batch_size: {}, batching: full={}/{}s incremental={}/{}s realtime={}/{}s global_age={}s",
            self.poll_interval,
            self.batch_size,
            self.batching_config.full_batch_size,
            self.batching_config.full_max_age_secs,
            self.batching_config.incremental_batch_size,
            self.batching_config.incremental_max_age_secs,
            self.batching_config.realtime_batch_size,
            self.batching_config.realtime_max_age_secs,
            self.batching_config.global_max_age_secs,
        );

        loop {
            tokio::select! {
                _ = poll_interval.tick() => {
                    if let Err(e) = self.process_batch_safe().await {
                        error!("Failed to process batch: {}", e);
                    }
                }
                _ = heartbeat_interval.tick() => {
                    if let Ok(stats) = self.event_queue.get_queue_stats().await {
                        info!(
                            "Queue stats - Pending: {}, Processing: {}, Completed: {}, Failed: {}, Dead Letter: {}",
                            stats.pending, stats.processing, stats.completed, stats.failed, stats.dead_letter
                        );
                    }
                }
                _ = retry_interval.tick() => {
                    if let Ok(retried) = self.event_queue.retry_failed_events().await {
                        if retried > 0 {
                            info!("Retried {} failed events", retried);
                        }
                    }
                }
                _ = cleanup_interval.tick() => {
                    if let Ok(result) = self.event_queue.cleanup_old_events(7).await {
                        if result.completed_deleted > 0 || result.dead_letter_deleted > 0 {
                            info!(
                                "Cleaned up old events - Completed: {}, Dead Letter: {}",
                                result.completed_deleted, result.dead_letter_deleted
                            );
                        }
                    }
                    // Cleanup embedding queue
                    if let Ok(deleted) = self.embedding_queue.cleanup_completed(7).await {
                        if deleted > 0 {
                            info!("Cleaned up {} old completed embedding queue items", deleted);
                        }
                    }
                    if let Ok(deleted) = self.embedding_queue.cleanup_failed(7).await {
                        if deleted > 0 {
                            info!("Cleaned up {} old failed embedding queue items", deleted);
                        }
                    }
                }
                _ = recovery_interval.tick() => {
                    // Periodic recovery of stale processing items
                    if let Ok(recovered) = self.event_queue.recover_stale_processing_items(300).await {
                        if recovered > 0 {
                            info!("Recovered {} stale processing items during periodic cleanup", recovered);
                        }
                    }
                    // Periodic recovery of stale embedding processing items
                    if let Ok(recovered) = self.embedding_queue.recover_stale_processing_items(300).await {
                        if recovered > 0 {
                            info!("Recovered {} stale embedding processing items during periodic cleanup", recovered);
                        }
                    }
                }
                _ = gc_interval.tick() => {
                    match gc_semaphore.clone().try_acquire_owned() {
                        Ok(permit) => {
                            let pool = self.state.db_pool.pool().clone();
                            let storage = self.state.content_storage.clone();
                            tokio::spawn(async move {
                                let _permit = permit;
                                let gc = ContentBlobGC::new(pool, storage, GCConfig::from_env());
                                match gc.run().await {
                                    Ok(result) => {
                                        if result.blobs_deleted > 0 {
                                            info!(
                                                "Content blob GC completed: deleted={}, bytes_reclaimed={}",
                                                result.blobs_deleted, result.bytes_reclaimed
                                            );
                                        }
                                    }
                                    Err(e) => {
                                        error!("Content blob GC failed: {}", e);
                                    }
                                }
                            });
                        }
                        Err(_) => {
                            debug!("Skipping GC tick: previous run still in progress");
                        }
                    }
                }
            }
        }
    }

    async fn process_batch_safe(&self) -> Result<()> {
        let _guard = self.processing_mutex.lock().await;
        self.process_batch().await
    }

    async fn process_batch(&self) -> Result<()> {
        // Cap iterations per invocation so a full queue cannot hold this future
        // for arbitrarily long, which would starve every other branch of the
        // main select! loop (GC, retry, stale-recovery, heartbeat). Subsequent
        // calls are driven by poll_interval in the main loop.
        const MAX_BATCHES_PER_CALL: usize = 3;

        // Sync-type-aware batching: only process if pending events meet a
        // threshold (size or age). This lets small incremental trickles
        // accumulate while full-sync bursts flow through quickly.
        let summary = self.event_queue.get_queue_summary().await?;
        let (by_sync_type, orphan_count) = summarize_pending(&summary);
        let ready = self
            .batching_config
            .ready_sync_types(&by_sync_type, orphan_count);

        let total_pending: i64 = by_sync_type.values().map(|(c, _)| *c).sum::<i64>() + orphan_count;
        if ready.is_empty() && orphan_count == 0 {
            if total_pending > 0 {
                debug!(
                    "Skipping batch: {} pending events do not meet sync-type thresholds",
                    total_pending
                );
            }
            return Ok(());
        }

        let mut total_processed = 0;
        let mut batches_dequeued = 0;

        // Process orphan events first (no valid sync_run). These mainly happen
        // in tests that enqueue directly without creating sync_run rows.
        while batches_dequeued < MAX_BATCHES_PER_CALL && orphan_count > 0 {
            let events = self
                .event_queue
                .dequeue_batch_orphans(self.batch_size)
                .await?;
            if events.is_empty() {
                break;
            }
            batches_dequeued += 1;
            total_processed += self.process_dequeued_events(events).await?;
        }

        for (sync_type, reason) in ready {
            if batches_dequeued >= MAX_BATCHES_PER_CALL {
                break;
            }

            info!(
                "Processing pending events for {:?}. Triggered by: {}",
                sync_type, reason
            );

            let remaining = MAX_BATCHES_PER_CALL - batches_dequeued;
            for _ in 0..remaining {
                let events = self
                    .event_queue
                    .dequeue_batch_by_sync_type(self.batch_size, sync_type)
                    .await?;
                if events.is_empty() {
                    break;
                }
                batches_dequeued += 1;
                total_processed += self.process_dequeued_events(events).await?;
            }
        }

        if total_processed > 0 {
            info!(
                "Processed {} events this call (cap reached, continuing next tick)",
                total_processed
            );
        }
        Ok(())
    }

    async fn process_dequeued_events(&self, events: Vec<ConnectorEventQueueItem>) -> Result<usize> {
        if events.is_empty() {
            return Ok(0);
        }

        info!(
            "Processing batch of {} events using batch operations",
            events.len()
        );

        // A single dequeue may contain events from multiple sync runs (e.g.
        // two simultaneous full syncs). Group by sync_run_id so that progress
        // tracking and per-sync-run batching remain correct.
        let mut by_sync_run: HashMap<String, Vec<ConnectorEventQueueItem>> = HashMap::new();
        for ev in events {
            by_sync_run
                .entry(ev.sync_run_id.clone())
                .or_default()
                .push(ev);
        }

        let mut total_processed = 0;

        for (sync_run_id, run_events) in by_sync_run {
            let batch_start_time = std::time::Instant::now();
            let events_clone = run_events.clone();
            let batch = self.group_events_by_type(sync_run_id, run_events).await?;

            if batch.is_empty() {
                continue;
            }

            info!(
                "Sync-run batch contains: {} upsert, {} deleted documents ({} upsert events, {} deleted events)",
                batch.documents_upsert.len(),
                batch.documents_deleted.len(),
                batch.documents_upsert.iter().map(|(_, event_ids)| event_ids.len()).sum::<usize>(),
                batch.documents_deleted.iter().map(|(_, _, event_ids)| event_ids.len()).sum::<usize>()
            );

            let batch_sync_run_id = batch.sync_run_id.clone();
            let result = self.process_event_batch(batch).await;

            match result {
                Ok(batch_result) => {
                    if !batch_result.successful_event_ids.is_empty() {
                        if let Err(e) = self
                            .event_queue
                            .mark_events_completed_batch(batch_result.successful_event_ids.clone())
                            .await
                        {
                            error!(
                                "Failed to mark {} events as completed: {}",
                                batch_result.successful_event_ids.len(),
                                e
                            );
                        }
                    }

                    if !batch_result.failed_events.is_empty() {
                        if let Err(e) = self
                            .event_queue
                            .mark_events_dead_letter_batch(batch_result.failed_events.clone())
                            .await
                        {
                            error!(
                                "Failed to mark {} events as failed: {}",
                                batch_result.failed_events.len(),
                                e
                            );
                        }
                    }

                    if batch_result.successful_documents_count > 0 {
                        if let Err(e) = self
                            .sync_run_repo
                            .increment_progress_by(
                                &batch_sync_run_id,
                                batch_result.successful_documents_count as i32,
                            )
                            .await
                        {
                            warn!(
                                "Failed to update sync run progress for {}: {}",
                                batch_sync_run_id, e
                            );
                        }
                    }

                    self.extract_and_upsert_people(&events_clone).await;

                    total_processed += batch_result.successful_event_ids.len();

                    let batch_duration = batch_start_time.elapsed();
                    info!(
                        "Sync-run batch processing completed: {} successful, {} failed (took {:?}, {:.1} events/sec)",
                        batch_result.successful_event_ids.len(),
                        batch_result.failed_events.len(),
                        batch_duration,
                        batch_result.successful_event_ids.len() as f64 / batch_duration.as_secs_f64()
                    );
                }
                Err(e) => {
                    error!("Batch processing failed: {}", e);
                    let err_msg = e.to_string();
                    let failed: Vec<(String, String)> = events_clone
                        .iter()
                        .map(|ev| (ev.id.clone(), err_msg.clone()))
                        .collect();
                    if let Err(mark_err) =
                        self.event_queue.mark_events_dead_letter_batch(failed).await
                    {
                        error!(
                            "Failed to mark {} events as failed after batch error: {}",
                            events_clone.len(),
                            mark_err
                        );
                    }
                }
            }
        }

        Ok(total_processed)
    }

    async fn group_events_by_type(
        &self,
        sync_run_id: String,
        events: Vec<ConnectorEventQueueItem>,
    ) -> Result<EventBatch> {
        let mut batch = EventBatch::new(sync_run_id);

        // Temporary storage for grouping events by document key
        // Single map for both creates and updates — both go through batch_upsert
        let mut upsert_docs: HashMap<String, (Document, Vec<String>)> = HashMap::new();
        let mut deleted_docs: HashMap<String, (String, String, Vec<String>)> = HashMap::new();

        for event_item in events {
            let event_id = event_item.id.clone();

            // Parse the event payload
            let event: ConnectorEvent = serde_json::from_value(event_item.payload.clone())?;

            match event {
                ConnectorEvent::DocumentCreated {
                    source_id,
                    document_id,
                    content_id,
                    metadata,
                    permissions,
                    attributes,
                    ..
                } => {
                    let document = self.create_document_from_event(
                        source_id.clone(),
                        document_id.clone(),
                        content_id,
                        metadata,
                        permissions,
                        attributes,
                    )?;

                    let key = format!("{}:{}", source_id, document_id);
                    upsert_docs
                        .entry(key)
                        .and_modify(|(_, event_ids)| event_ids.push(event_id.clone()))
                        .or_insert_with(|| (document, vec![event_id]));
                }
                ConnectorEvent::DocumentUpdated {
                    source_id,
                    document_id,
                    content_id,
                    metadata,
                    permissions,
                    attributes,
                    ..
                } => {
                    // Build document the same way as creates — batch_upsert's
                    // COALESCE handles preserving existing values when
                    // permissions/attributes are NULL
                    let has_permissions = permissions.is_some();
                    let document = self.create_document_from_event(
                        source_id.clone(),
                        document_id.clone(),
                        content_id,
                        metadata,
                        permissions.unwrap_or(DocumentPermissions {
                            public: false,
                            users: vec![],
                            groups: vec![],
                        }),
                        attributes,
                    )?;

                    // For updates with no permissions, set to Null so COALESCE
                    // preserves existing DB values
                    let mut document = document;
                    if !has_permissions {
                        document.permissions = serde_json::Value::Null;
                    }

                    let key = format!("{}:{}", source_id, document_id);
                    upsert_docs
                        .entry(key)
                        .and_modify(|(_, event_ids)| event_ids.push(event_id.clone()))
                        .or_insert_with(|| (document, vec![event_id]));
                }
                ConnectorEvent::DocumentDeleted {
                    source_id,
                    document_id,
                    ..
                } => {
                    let key = format!("{}:{}", source_id, document_id);
                    deleted_docs
                        .entry(key)
                        .and_modify(|(_, _, event_ids)| event_ids.push(event_id.clone()))
                        .or_insert_with(|| (source_id, document_id, vec![event_id]));
                }
                ConnectorEvent::GroupMembershipSync {
                    source_id,
                    group_email,
                    group_name,
                    member_emails,
                    ..
                } => {
                    let key = format!("{}:{}", source_id, group_email);
                    if let Some(existing) = batch
                        .group_syncs
                        .iter_mut()
                        .find(|g| format!("{}:{}", g.source_id, g.group_email) == key)
                    {
                        existing.member_emails = member_emails;
                        existing.group_name = group_name;
                        existing.event_ids.push(event_id);
                    } else {
                        batch.group_syncs.push(GroupSyncEvent {
                            source_id,
                            group_email,
                            group_name,
                            member_emails,
                            event_ids: vec![event_id],
                        });
                    }
                }
            }
        }

        batch.documents_upsert = upsert_docs.into_values().collect();
        batch.documents_deleted = deleted_docs.into_values().collect();

        Ok(batch)
    }

    async fn process_event_batch(&self, batch: EventBatch) -> Result<BatchProcessingResult> {
        let mut result = BatchProcessingResult::new();

        // Process document upserts (creates + updates) in a single batch
        if !batch.documents_upsert.is_empty() {
            let docs_count = batch.documents_upsert.len();
            match self
                .process_documents_upsert_batch(&batch.documents_upsert)
                .await
            {
                Ok(successful_ids) => {
                    result.successful_event_ids.extend(successful_ids);
                    result.successful_documents_count += docs_count;
                }
                Err(e) => {
                    error!("Batch document upsert failed: {}", e);
                    for (_, event_ids) in batch.documents_upsert {
                        for event_id in event_ids {
                            result.failed_events.push((event_id, e.to_string()));
                        }
                    }
                }
            }
        }

        // Process document deletions in batch
        if !batch.documents_deleted.is_empty() {
            let docs_count = batch.documents_deleted.len();
            match self
                .process_documents_deleted_batch(&batch.documents_deleted)
                .await
            {
                Ok(successful_ids) => {
                    result.successful_event_ids.extend(successful_ids);
                    result.successful_documents_count += docs_count;
                }
                Err(e) => {
                    error!("Batch document deletion failed: {}", e);
                    // Add all deletion events to failed list
                    for (_, _, event_ids) in batch.documents_deleted {
                        for event_id in event_ids {
                            result.failed_events.push((event_id, e.to_string()));
                        }
                    }
                }
            }
        }

        // Process group membership syncs
        if !batch.group_syncs.is_empty() {
            let group_count = batch.group_syncs.len();
            info!("Processing {} group membership sync events", group_count);
            let group_repo = GroupRepository::new(self.state.db_pool.pool());

            for group_sync in batch.group_syncs {
                match self
                    .process_group_membership_sync(&group_repo, &group_sync)
                    .await
                {
                    Ok(()) => {
                        result.successful_event_ids.extend(group_sync.event_ids);
                    }
                    Err(e) => {
                        error!(
                            "Group membership sync failed for {}: {}",
                            group_sync.group_email, e
                        );
                        for event_id in group_sync.event_ids {
                            result.failed_events.push((event_id, e.to_string()));
                        }
                    }
                }
            }
        }

        Ok(result)
    }

    async fn process_group_membership_sync(
        &self,
        group_repo: &GroupRepository,
        sync_event: &GroupSyncEvent,
    ) -> Result<()> {
        let group = group_repo
            .upsert_group(
                &sync_event.source_id,
                &sync_event.group_email,
                sync_event.group_name.as_deref(),
                None,
            )
            .await
            .context("Failed to upsert group")?;

        let member_count = group_repo
            .sync_group_members(&group.id, &sync_event.member_emails)
            .await
            .context("Failed to sync group members")?;

        info!(
            "Synced group {} ({}) with {} members",
            sync_event.group_email, group.id, member_count
        );

        Ok(())
    }

    async fn extract_and_upsert_people(&self, events: &[ConnectorEventQueueItem]) {
        let person_repo = PersonRepository::new(self.state.db_pool.pool());

        let mut manifest_cache: HashMap<String, shared::models::ConnectorManifest> = HashMap::new();
        let mut seen: HashMap<String, shared::PersonUpsert> = HashMap::new();

        for event_item in events {
            let event: ConnectorEvent = match serde_json::from_value(event_item.payload.clone()) {
                Ok(e) => e,
                Err(_) => continue,
            };

            let source_id = event.source_id().to_string();

            // Look up manifest for this source's connector (cached per batch)
            if !manifest_cache.contains_key(&source_id) {
                if let Some(m) = self.load_manifest_for_source(&source_id).await {
                    manifest_cache.insert(source_id.clone(), m);
                }
            }
            let manifest = manifest_cache.get(&source_id);

            let (extra_schema, attributes_schema, search_operators) = match manifest {
                Some(m) => (
                    m.extra_schema.as_ref(),
                    m.attributes_schema.as_ref(),
                    m.search_operators.as_slice(),
                ),
                None => (None, None, &[] as &[shared::models::SearchOperator]),
            };

            let people = people_extractor::extract_people(
                extra_schema,
                attributes_schema,
                search_operators,
                &event,
            );

            for person in people {
                seen.entry(person.email.clone())
                    .or_insert_with(|| shared::PersonUpsert {
                        email: person.email,
                        display_name: person.display_name,
                    });
            }
        }

        if seen.is_empty() {
            return;
        }

        let people: Vec<shared::PersonUpsert> = seen.into_values().collect();
        let count = people.len();

        match person_repo.upsert_people_batch(&people).await {
            Ok(_) => {
                debug!("Upserted {} people from batch", count);
            }
            Err(e) => {
                error!("Failed to upsert people: {}", e);
            }
        }
    }

    async fn load_manifest_for_source(
        &self,
        source_id: &str,
    ) -> Option<shared::models::ConnectorManifest> {
        // Look up source_type from the sources table
        let source_type: String =
            sqlx::query_scalar("SELECT source_type FROM sources WHERE id = $1")
                .bind(source_id)
                .fetch_optional(self.state.db_pool.pool())
                .await
                .ok()??;

        // Read cached manifest from Redis: connector:manifest:{source_type}
        let key = format!("connector:manifest:{}", source_type);
        let mut conn = self
            .state
            .redis_client
            .get_multiplexed_async_connection()
            .await
            .ok()?;
        let json: String = redis::AsyncCommands::get(&mut conn, &key).await.ok()?;
        serde_json::from_str(&json).ok()
    }

    // Helper methods for batch processing
    fn convert_metadata_to_json(&self, metadata: &DocumentMetadata) -> Result<serde_json::Value> {
        let mut metadata_json = serde_json::to_value(metadata)?;

        // Convert size from string to number if present
        if let Some(size_str) = &metadata.size {
            if let Ok(size_num) = size_str.parse::<i64>() {
                if let Some(obj) = metadata_json.as_object_mut() {
                    obj.insert(
                        "size".to_string(),
                        serde_json::Value::Number(size_num.into()),
                    );
                }
            }
        }

        Ok(metadata_json)
    }

    fn create_document_from_event(
        &self,
        source_id: String,
        document_id: String,
        content_id: String,
        metadata: DocumentMetadata,
        permissions: DocumentPermissions,
        attributes: Option<DocumentAttributes>,
    ) -> Result<Document> {
        let now = sqlx::types::time::OffsetDateTime::now_utc();
        let metadata_json = self.convert_metadata_to_json(&metadata)?;
        let permissions_json = serde_json::to_value(&permissions)?;
        let attributes_json = attributes
            .map(|a| serde_json::to_value(&a))
            .transpose()?
            .unwrap_or(serde_json::json!({}));

        // Extract file extension from URL or mime type
        let file_extension = metadata.url.as_ref().and_then(|url| {
            url.split('.')
                .last()
                .filter(|ext| !ext.contains('/') && !ext.contains('?'))
                .map(|ext| ext.to_lowercase())
        });

        // Parse file size from string to i64
        let file_size = metadata
            .size
            .as_ref()
            .and_then(|size_str| size_str.parse::<i64>().ok());

        // Ensure last_indexed_at is after created_at
        let last_indexed_at = now + std::time::Duration::from_millis(1);

        Ok(Document {
            id: ulid::Ulid::new().to_string(),
            source_id,
            external_id: document_id,
            title: metadata.title.unwrap_or_else(|| "Untitled".to_string()),
            content_id: Some(content_id),
            content_type: metadata.content_type.or(metadata.mime_type),
            file_size,
            file_extension,
            url: metadata.url,
            metadata: metadata_json,
            permissions: permissions_json,
            attributes: attributes_json,
            created_at: now,
            updated_at: now,
            last_indexed_at,
        })
    }

    async fn process_documents_upsert_batch(
        &self,
        documents_with_event_ids: &[(Document, Vec<String>)],
    ) -> Result<Vec<String>> {
        let start_time = std::time::Instant::now();
        let documents: Vec<Document> = documents_with_event_ids
            .iter()
            .map(|(doc, _)| doc.clone())
            .collect();

        // Batch fetch content from storage
        let content_fetch_start = std::time::Instant::now();
        let content_ids: Vec<String> = documents
            .iter()
            .filter_map(|d| d.content_id.clone())
            .collect();

        let content_map = self
            .state
            .content_storage
            .batch_get_text(content_ids)
            .await?;

        // Build contents vector in the same order as documents
        let contents: Vec<String> = documents
            .iter()
            .map(|doc| {
                doc.content_id
                    .as_ref()
                    .and_then(|cid| content_map.get(cid).cloned())
                    .with_context(|| format!("Failed to get content for document {}", doc.id))
            })
            .collect::<Result<Vec<_>>>()?;

        debug!(
            "Batch fetched content for {} documents in {:?}",
            documents.len(),
            content_fetch_start.elapsed()
        );

        let repo = DocumentRepository::new(self.state.db_pool.pool());

        // Batch upsert documents with content
        let upsert_start = std::time::Instant::now();
        let upserted_documents = repo.batch_upsert(documents, contents).await?;
        debug!(
            "Batch upsert of {} documents took {:?}",
            upserted_documents.len(),
            upsert_start.elapsed()
        );

        // Batch add documents to embedding queue
        let embedding_start = std::time::Instant::now();
        let doc_ids_for_embedding: Vec<String> =
            upserted_documents.iter().map(|d| d.id.clone()).collect();
        if !doc_ids_for_embedding.is_empty() {
            if let Err(e) = self
                .state
                .embedding_queue
                .enqueue_batch(doc_ids_for_embedding.clone())
                .await
            {
                error!(
                    "Failed to batch queue embeddings for {} documents: {}",
                    doc_ids_for_embedding.len(),
                    e
                );
            }
        }
        debug!(
            "Embedding queue batch operation took {:?}",
            embedding_start.elapsed()
        );

        let total_duration = start_time.elapsed();
        info!(
            "Batch processed {} documents successfully (took {:?}, {:.1} docs/sec)",
            upserted_documents.len(),
            total_duration,
            upserted_documents.len() as f64 / total_duration.as_secs_f64()
        );

        // Return all the event IDs that were successful
        Ok(documents_with_event_ids
            .iter()
            .flat_map(|(_, event_ids)| event_ids.clone())
            .collect())
    }

    async fn process_documents_deleted_batch(
        &self,
        deletions: &[(String, String, Vec<String>)], // (source_id, document_id, event_ids)
    ) -> Result<Vec<String>> {
        let start_time = std::time::Instant::now();
        let repo = DocumentRepository::new(self.state.db_pool.pool());
        let embedding_repo = EmbeddingRepository::new(self.state.db_pool.pool());

        // All deletion events are considered successful (even if doc not found)
        let successful_event_ids: Vec<String> = deletions
            .iter()
            .flat_map(|(_, _, event_ids)| event_ids.clone())
            .collect();

        // Batch-lookup all documents by (source_id, external_id)
        let pairs: Vec<(String, String)> = deletions
            .iter()
            .map(|(source_id, document_id, _)| (source_id.clone(), document_id.clone()))
            .collect();

        let found_documents = repo.find_by_external_ids(&pairs).await?;
        let document_ids_to_delete: Vec<String> =
            found_documents.iter().map(|d| d.id.clone()).collect();

        if found_documents.len() < deletions.len() {
            warn!(
                "{} of {} documents not found for deletion (already deleted?)",
                deletions.len() - found_documents.len(),
                deletions.len()
            );
        }

        if !document_ids_to_delete.is_empty() {
            // Delete embeddings in batch
            if let Err(e) = embedding_repo
                .bulk_delete_by_document_ids(&document_ids_to_delete)
                .await
            {
                error!(
                    "Failed to batch delete embeddings for {} documents: {}",
                    document_ids_to_delete.len(),
                    e
                );
            }

            // Delete documents in batch
            let delete_start = std::time::Instant::now();
            let deleted_count = repo.batch_delete(document_ids_to_delete.clone()).await?;
            debug!("Batch document deletion took {:?}", delete_start.elapsed());

            let total_duration = start_time.elapsed();
            info!(
                "Batch deleted {} documents and their embeddings (took {:?})",
                deleted_count, total_duration
            );
        }

        Ok(successful_event_ids)
    }
}
