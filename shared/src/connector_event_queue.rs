//! Connector-event adapter for the generic PostgreSQL task queue.
//!
//! This module owns connector routing, payload validation, batching and
//! summaries. Lease, retry, fencing and lifecycle state transitions remain in
//! [`crate::task_queue::TaskQueue`].

use anyhow::{Context, Result, bail};
use serde_json::Value;
use sqlx::{PgPool, Row};
use time::OffsetDateTime;

use crate::models::{ConnectorEvent, EventStatus, SyncType};
use crate::task_queue::{ClaimOptions, EnqueueTaskRequest, Task, TaskClaim, TaskQueue, TaskStatus};

pub const CONNECTOR_EVENT_TASK_TYPE: &str = "connector_event";
pub const CONNECTOR_EVENT_PAYLOAD_VERSION: i32 = 1;
pub const CONNECTOR_EVENT_MAX_ATTEMPTS: i32 = 3;
pub const CONNECTOR_EVENT_LEASE_SECONDS: i32 = 300;
pub const CONNECTOR_EVENT_RETRY_DELAY_SECONDS: i32 = 300;
const CONTENT_ID_LENGTH: usize = 26;

fn event_type(event: &ConnectorEvent) -> &'static str {
    match event {
        ConnectorEvent::DocumentCreated { .. } => "document_created",
        ConnectorEvent::DocumentUpdated { .. } => "document_updated",
        ConnectorEvent::DocumentDeleted { .. } => "document_deleted",
        ConnectorEvent::GroupMembershipSync { .. } => "group_membership_sync",
        ConnectorEvent::PersonSync { .. } => "person_sync",
        ConnectorEvent::PersonDeleted { .. } => "person_deleted",
    }
}

fn is_person_event_type(event_type: &str) -> bool {
    matches!(event_type, "person_sync" | "person_deleted")
}

/// A validated connector task. `source_id`, `sync_run_id`, and `event_type`
/// are derived from `event`; they are not independent queue columns.
#[derive(Debug, Clone)]
pub struct ConnectorEventQueueItem {
    pub id: String,
    pub event: ConnectorEvent,
    pub source_id: String,
    pub sync_run_id: String,
    pub event_type: String,
    /// Retained for callers that need to inspect the exact versioned payload.
    pub payload: Value,
    pub status: EventStatus,
    pub retry_count: i32,
    pub max_retries: i32,
    pub created_at: OffsetDateTime,
    pub processed_at: Option<OffsetDateTime>,
    pub error_message: Option<String>,
    pub claim_token: Option<String>,
    pub weight: i64,
}

/// A batch claim owns all of its events with one fencing token.
#[derive(Debug, Clone)]
pub struct ConnectorEventClaim {
    pub claim_token: String,
    pub events: Vec<ConnectorEventQueueItem>,
}

#[derive(Clone)]
pub struct ConnectorEventQueue {
    pool: PgPool,
    task_queue: TaskQueue,
}

impl ConnectorEventQueue {
    pub fn new(pool: PgPool) -> Self {
        Self {
            task_queue: TaskQueue::new(pool.clone()),
            pool,
        }
    }

    pub async fn enqueue(&self, source_id: &str, event: &ConnectorEvent) -> Result<String> {
        let ids = self
            .enqueue_batch(source_id, std::slice::from_ref(event))
            .await?;
        ids.into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("connector event was not enqueued"))
    }

    pub async fn enqueue_batch(
        &self,
        source_id: &str,
        events: &[ConnectorEvent],
    ) -> Result<Vec<String>> {
        if events.is_empty() {
            return Ok(Vec::new());
        }
        if source_id.trim().is_empty() {
            bail!("connector event source_id must not be empty");
        }

        let payloads: Vec<Value> = events
            .iter()
            .map(serde_json::to_value)
            .collect::<serde_json::Result<Vec<_>>>()?;
        let mut content_ids = Vec::new();
        for event in events {
            if event.source_id() != source_id {
                bail!("connector event source_id does not match enqueue context");
            }
            if let Some(content_id) = content_id(event) {
                if content_id.len() == CONTENT_ID_LENGTH {
                    content_ids.push(content_id.to_string());
                }
            }
        }
        content_ids.sort_unstable();
        content_ids.dedup();
        let content_sizes: std::collections::HashMap<String, i64> = if content_ids.is_empty() {
            std::collections::HashMap::new()
        } else {
            sqlx::query(
                "SELECT id::text AS id, size_bytes FROM content_blobs WHERE id::text = ANY($1)",
            )
            .bind(&content_ids)
            .fetch_all(&self.pool)
            .await?
            .into_iter()
            .map(|row| (row.get("id"), row.get("size_bytes")))
            .collect()
        };

        let available_at = OffsetDateTime::now_utc();
        let tasks: Vec<EnqueueTaskRequest> = events
            .iter()
            .zip(payloads)
            .map(|(event, payload)| {
                let mut task = EnqueueTaskRequest::new(CONNECTOR_EVENT_TASK_TYPE, payload.clone());
                task.payload_version = CONNECTOR_EVENT_PAYLOAD_VERSION;
                task.available_at = available_at;
                task.max_attempts = CONNECTOR_EVENT_MAX_ATTEMPTS;
                task.weight = serialized_weight(&payload)
                    + content_id(event)
                        .and_then(|id| content_sizes.get(id).copied())
                        .unwrap_or(0);
                task.concurrency_key = person_concurrency_key(event);
                task
            })
            .collect();
        Ok(self
            .task_queue
            .enqueue_bulk(&tasks)
            .await?
            .into_iter()
            .map(|task| task.id)
            .collect())
    }

    pub async fn claim_batch(
        &self,
        batch_size: i32,
        max_bytes: i64,
    ) -> Result<ConnectorEventClaim> {
        let mut tx = self.pool.begin().await?;
        let ids = self
            .candidate_ids(
                &mut *tx,
                "(q.payload ->> 'type' NOT IN ('person_sync', 'person_deleted') OR q.payload ->> 'type' IS NULL)",
                batch_size,
            )
            .await?;
        let claim = self
            .claim_ids_in_transaction(&mut tx, ids, batch_size, max_bytes)
            .await?;
        tx.commit().await?;
        self.map_claim(claim).await
    }

    pub async fn claim_person_mutations(
        &self,
        batch_size: i32,
        max_bytes: i64,
    ) -> Result<ConnectorEventClaim> {
        let mut tx = self.pool.begin().await?;
        let ids = self
            .candidate_ids(
                &mut *tx,
                "(q.payload ->> 'type' IN ('person_sync', 'person_deleted') OR q.payload ->> 'type' IS NULL) AND (q.concurrency_key IS NULL OR NOT EXISTS (SELECT 1 FROM tasks older WHERE older.task_type = q.task_type AND older.concurrency_key = q.concurrency_key AND older.status IN ('pending', 'running') AND older.id < q.id))",
                batch_size,
            )
            .await?;
        let claim = self
            .claim_ids_in_transaction(&mut tx, ids, batch_size, max_bytes)
            .await?;
        tx.commit().await?;
        self.map_claim(claim).await
    }

    pub async fn claim_batch_by_sync_type(
        &self,
        batch_size: i32,
        sync_type: SyncType,
        max_bytes: i64,
    ) -> Result<ConnectorEventClaim> {
        let mut tx = self.pool.begin().await?;
        let rows = sqlx::query(
            r#"
            SELECT q.id
            FROM tasks q
            JOIN sync_runs s ON s.id = q.payload ->> 'sync_run_id'
            WHERE q.task_type = $1
              AND q.status = 'pending'
              AND q.available_at <= NOW()
              AND (q.payload ->> 'type' NOT IN ('person_sync', 'person_deleted') OR q.payload ->> 'type' IS NULL)
              AND s.sync_type = $2
            ORDER BY q.id
            LIMIT $3
            FOR UPDATE OF q SKIP LOCKED
            "#,
        )
        .bind(CONNECTOR_EVENT_TASK_TYPE)
        .bind(sync_type.to_string())
        .bind(batch_size)
        .fetch_all(&mut *tx)
        .await?;
        let claim = self
            .claim_ids_in_transaction(
                &mut tx,
                rows.into_iter().map(|row| row.get("id")).collect(),
                batch_size,
                max_bytes,
            )
            .await?;
        tx.commit().await?;
        self.map_claim(claim).await
    }

    pub async fn claim_orphans(
        &self,
        batch_size: i32,
        max_bytes: i64,
    ) -> Result<ConnectorEventClaim> {
        let mut tx = self.pool.begin().await?;
        let rows = sqlx::query(
            r#"
            SELECT q.id
            FROM tasks q
            LEFT JOIN sync_runs s ON s.id = q.payload ->> 'sync_run_id'
            WHERE q.task_type = $1
              AND q.status = 'pending'
              AND q.available_at <= NOW()
              AND (q.payload ->> 'type' NOT IN ('person_sync', 'person_deleted') OR q.payload ->> 'type' IS NULL)
              AND s.id IS NULL
            ORDER BY q.id
            LIMIT $2
            FOR UPDATE OF q SKIP LOCKED
            "#,
        )
        .bind(CONNECTOR_EVENT_TASK_TYPE)
        .bind(batch_size)
        .fetch_all(&mut *tx)
        .await?;
        let claim = self
            .claim_ids_in_transaction(
                &mut tx,
                rows.into_iter().map(|row| row.get("id")).collect(),
                batch_size,
                max_bytes,
            )
            .await?;
        tx.commit().await?;
        self.map_claim(claim).await
    }

    async fn candidate_ids<'e, E>(
        &self,
        executor: E,
        predicate: &str,
        limit: i32,
    ) -> Result<Vec<String>>
    where
        E: sqlx::Executor<'e, Database = sqlx::Postgres>,
    {
        let query = format!(
            "SELECT q.id FROM tasks q WHERE q.task_type = $1 AND q.status = 'pending' AND q.available_at <= NOW() AND {} ORDER BY q.id LIMIT $2 FOR UPDATE OF q SKIP LOCKED",
            predicate
        );
        Ok(sqlx::query(&query)
            .bind(CONNECTOR_EVENT_TASK_TYPE)
            .bind(limit)
            .fetch_all(executor)
            .await?
            .into_iter()
            .map(|row| row.get("id"))
            .collect())
    }

    async fn claim_ids_in_transaction(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        ids: Vec<String>,
        batch_size: i32,
        max_bytes: i64,
    ) -> Result<TaskClaim> {
        self.task_queue
            .claim_bulk(
                &mut **tx,
                CONNECTOR_EVENT_TASK_TYPE,
                "indexer",
                &ClaimOptions {
                    candidate_ids: Some(ids),
                    limit: batch_size,
                    max_weight: Some(max_bytes),
                    lease_seconds: CONNECTOR_EVENT_LEASE_SECONDS,
                    ..Default::default()
                },
            )
            .await
    }

    async fn map_claim(&self, claim: TaskClaim) -> Result<ConnectorEventClaim> {
        let mut events = Vec::with_capacity(claim.tasks.len());
        let mut malformed: Vec<(String, String)> = Vec::new();
        for task in claim.tasks {
            let task_id = task.id.clone();
            match task_to_item(task) {
                Ok(item) => events.push(item),
                Err(error) => malformed.push((task_id, error.to_string())),
            }
        }
        if !malformed.is_empty() {
            // Malformed claimed rows are terminal so they cannot block a
            // concurrency key or repeatedly consume worker time.
            let failures = malformed;
            if !failures.is_empty() {
                self.task_queue
                    .fail_bulk_with_errors(&failures, &claim.claim_token, false, 0)
                    .await?;
            }
        }
        Ok(ConnectorEventClaim {
            claim_token: claim.claim_token,
            events,
        })
    }

    // Compatibility-shaped dequeue methods are adapters over a real generic
    // claim. New workers should use the claim methods to retain the token.
    pub async fn dequeue_batch(&self, batch_size: i32) -> Result<Vec<ConnectorEventQueueItem>> {
        Ok(self.claim_batch(batch_size, i64::MAX).await?.events)
    }

    pub async fn dequeue_batch_with_max_bytes(
        &self,
        batch_size: i32,
        max_bytes: i64,
    ) -> Result<Vec<ConnectorEventQueueItem>> {
        Ok(self.claim_batch(batch_size, max_bytes).await?.events)
    }

    pub async fn dequeue_person_mutations_with_max_bytes(
        &self,
        batch_size: i32,
        max_bytes: i64,
    ) -> Result<Vec<ConnectorEventQueueItem>> {
        Ok(self
            .claim_person_mutations(batch_size, max_bytes)
            .await?
            .events)
    }

    pub async fn dequeue_batch_by_sync_type(
        &self,
        batch_size: i32,
        sync_type: SyncType,
    ) -> Result<Vec<ConnectorEventQueueItem>> {
        Ok(self
            .claim_batch_by_sync_type(batch_size, sync_type, i64::MAX)
            .await?
            .events)
    }

    pub async fn dequeue_batch_by_sync_type_with_max_bytes(
        &self,
        batch_size: i32,
        sync_type: SyncType,
        max_bytes: i64,
    ) -> Result<Vec<ConnectorEventQueueItem>> {
        Ok(self
            .claim_batch_by_sync_type(batch_size, sync_type, max_bytes)
            .await?
            .events)
    }

    pub async fn dequeue_batch_orphans(
        &self,
        batch_size: i32,
    ) -> Result<Vec<ConnectorEventQueueItem>> {
        Ok(self.claim_orphans(batch_size, i64::MAX).await?.events)
    }

    pub async fn dequeue_batch_orphans_with_max_bytes(
        &self,
        batch_size: i32,
        max_bytes: i64,
    ) -> Result<Vec<ConnectorEventQueueItem>> {
        Ok(self.claim_orphans(batch_size, max_bytes).await?.events)
    }

    pub async fn mark_completed(&self, event_id: &str) -> Result<()> {
        let token = self.claim_token_for(event_id).await?;
        let completed = self
            .task_queue
            .complete_bulk(&[event_id.to_string()], &token)
            .await?;
        if completed != 1 {
            bail!("connector event completion was fenced for {}", event_id);
        }
        Ok(())
    }

    pub async fn mark_failed(&self, event_id: &str, error: &str) -> Result<()> {
        let token = self.claim_token_for(event_id).await?;
        let failed = self
            .task_queue
            .fail_bulk_with_errors(
                &[(event_id.to_string(), error.to_string())],
                &token,
                true,
                CONNECTOR_EVENT_RETRY_DELAY_SECONDS,
            )
            .await?;
        if failed.len() != 1 {
            bail!("connector event failure was fenced for {}", event_id);
        }
        Ok(())
    }

    pub async fn mark_events_completed_batch(&self, event_ids: Vec<String>) -> Result<i64> {
        if event_ids.is_empty() {
            return Ok(0);
        }
        let token = self.claim_token_for_ids(&event_ids).await?;
        Ok(self.task_queue.complete_bulk(&event_ids, &token).await?)
    }

    pub async fn mark_events_failed_batch(
        &self,
        event_ids_with_errors: Vec<(String, String)>,
    ) -> Result<i64> {
        if event_ids_with_errors.is_empty() {
            return Ok(0);
        }
        let ids: Vec<String> = event_ids_with_errors
            .iter()
            .map(|(id, _)| id.clone())
            .collect();
        let token = self.claim_token_for_ids(&ids).await?;
        Ok(self
            .task_queue
            .fail_bulk_with_errors(
                &event_ids_with_errors,
                &token,
                true,
                CONNECTOR_EVENT_RETRY_DELAY_SECONDS,
            )
            .await?
            .len() as i64)
    }

    pub async fn mark_events_dead_letter_batch(
        &self,
        event_ids_with_errors: Vec<(String, String)>,
    ) -> Result<i64> {
        if event_ids_with_errors.is_empty() {
            return Ok(0);
        }
        let ids: Vec<String> = event_ids_with_errors
            .iter()
            .map(|(id, _)| id.clone())
            .collect();
        let token = self.claim_token_for_ids(&ids).await?;
        Ok(self
            .task_queue
            .fail_bulk_with_errors(&event_ids_with_errors, &token, false, 0)
            .await?
            .len() as i64)
    }

    async fn claim_token_for(&self, event_id: &str) -> Result<String> {
        self.claim_token_for_ids(&[event_id.to_string()]).await
    }

    async fn claim_token_for_ids(&self, event_ids: &[String]) -> Result<String> {
        sqlx::query_scalar(
            "SELECT claim_token FROM tasks WHERE id = ANY($1) AND task_type = $2 AND status = 'running' LIMIT 1",
        )
        .bind(event_ids)
        .bind(CONNECTOR_EVENT_TASK_TYPE)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| anyhow::anyhow!("connector events are not currently claimed"))
    }

    pub async fn recover_stale_processing_items(&self, timeout_seconds: i32) -> Result<i64> {
        if timeout_seconds < 1 {
            bail!("recovery timeout must be >= 1");
        }
        Ok(
            sqlx::query("SELECT * FROM task_recover_stale_for_type($1, $2)")
                .bind(CONNECTOR_EVENT_TASK_TYPE)
                .bind(timeout_seconds)
                .fetch_all(&self.pool)
                .await?
                .len() as i64,
        )
    }

    pub async fn retry_failed_events(&self) -> Result<i64> {
        // Retryable failures are represented as delayed pending tasks by the
        // generic failure function; no connector-specific timer is needed.
        Ok(0)
    }

    pub async fn get_queue_stats(&self) -> Result<QueueStats> {
        let stats = self
            .task_queue
            .stats(Some(CONNECTOR_EVENT_TASK_TYPE))
            .await?;
        let mut result = QueueStats::default();
        for stat in stats {
            match stat.status {
                TaskStatus::Pending => result.pending = stat.count,
                TaskStatus::Running => result.processing = stat.count,
                TaskStatus::Completed => result.completed = stat.count,
                TaskStatus::DeadLetter => result.dead_letter = stat.count,
            }
        }
        Ok(result)
    }

    pub async fn get_queue_summary(&self) -> Result<QueueSummary> {
        self.get_queue_summary_filtered(false).await
    }

    pub async fn get_non_person_queue_summary(&self) -> Result<QueueSummary> {
        self.get_queue_summary_filtered(true).await
    }

    async fn get_queue_summary_filtered(&self, exclude_person: bool) -> Result<QueueSummary> {
        let rows = sqlx::query(
            r#"
            SELECT s.sync_type, q.status, COUNT(*) AS count,
                   MIN(q.created_at) AS oldest,
                   COALESCE(SUM(q.weight), 0)::BIGINT AS size_bytes,
                   COALESCE(BOOL_OR(s.status = 'completed'), false) AS has_completed_sync
            FROM tasks q
            LEFT JOIN sync_runs s ON s.id = q.payload ->> 'sync_run_id'
            WHERE q.task_type = $1
              AND (q.status <> 'pending' OR q.available_at <= NOW())
              AND (NOT $2 OR q.payload ->> 'type' NOT IN ('person_sync', 'person_deleted') OR q.payload ->> 'type' IS NULL)
            GROUP BY s.sync_type, q.status
            "#,
        )
        .bind(CONNECTOR_EVENT_TASK_TYPE)
        .bind(exclude_person)
        .fetch_all(&self.pool)
        .await?;

        let entries = rows
            .into_iter()
            .filter_map(|row| {
                let sync_type = match row
                    .try_get::<Option<String>, _>("sync_type")
                    .ok()?
                    .as_deref()
                {
                    None => None,
                    Some("full") => Some(SyncType::Full),
                    Some("incremental") => Some(SyncType::Incremental),
                    Some("realtime") => Some(SyncType::Realtime),
                    Some(_) => return None,
                };
                let status = match row.try_get::<String, _>("status").ok()?.as_str() {
                    "pending" => EventStatus::Pending,
                    "running" => EventStatus::Processing,
                    "completed" => EventStatus::Completed,
                    "dead_letter" => EventStatus::DeadLetter,
                    _ => return None,
                };
                Some(QueueSummaryEntry {
                    sync_type,
                    status,
                    count: row.try_get("count").ok()?,
                    oldest: row.try_get("oldest").ok(),
                    size_bytes: row.try_get("size_bytes").ok()?,
                    has_completed_sync: row.try_get("has_completed_sync").ok()?,
                })
            })
            .collect();
        Ok(QueueSummary { entries })
    }

    pub async fn get_pending_count(&self) -> Result<i64> {
        Ok(sqlx::query_scalar(
            "SELECT COUNT(*) FROM tasks WHERE task_type = $1 AND status = 'pending' AND available_at <= NOW()",
        )
        .bind(CONNECTOR_EVENT_TASK_TYPE)
        .fetch_one(&self.pool)
        .await?)
    }

    pub async fn cleanup_old_events(&self, retention_days: i32) -> Result<CleanupResult> {
        if retention_days < 0 {
            bail!("retention_days must be non-negative");
        }
        let cutoff = OffsetDateTime::now_utc() - time::Duration::days(i64::from(retention_days));
        let deleted: i64 = sqlx::query_scalar("SELECT task_cleanup_for_type($1, $2)")
            .bind(CONNECTOR_EVENT_TASK_TYPE)
            .bind(cutoff)
            .fetch_one(&self.pool)
            .await?;
        Ok(CleanupResult {
            completed_deleted: deleted as u64,
            dead_letter_deleted: 0,
        })
    }

    pub async fn heartbeat_bulk(
        &self,
        event_ids: &[String],
        claim_token: &str,
    ) -> Result<Vec<String>> {
        self.task_queue
            .heartbeat_bulk(event_ids, claim_token, CONNECTOR_EVENT_LEASE_SECONDS)
            .await
    }

    /// Classify IDs omitted by a heartbeat. A task may have completed between
    /// the worker's snapshot and the heartbeat update; that is not ownership
    /// loss and must not stop processing the rest of the batch.
    pub async fn classify_unrenewed(
        &self,
        event_ids: &[String],
    ) -> Result<(Vec<String>, Vec<String>)> {
        if event_ids.is_empty() {
            return Ok((Vec::new(), Vec::new()));
        }
        let rows = sqlx::query("SELECT id, status FROM tasks WHERE id = ANY($1)")
            .bind(event_ids)
            .fetch_all(&self.pool)
            .await?;
        let statuses: std::collections::HashMap<String, String> = rows
            .into_iter()
            .map(|row| (row.get("id"), row.get("status")))
            .collect();
        let mut terminal = Vec::new();
        let mut lost = Vec::new();
        for event_id in event_ids {
            match statuses.get(event_id).map(String::as_str) {
                Some("completed") | Some("dead_letter") => terminal.push(event_id.clone()),
                _ => lost.push(event_id.clone()),
            }
        }
        Ok((terminal, lost))
    }

    pub async fn complete_bulk(&self, event_ids: &[String], claim_token: &str) -> Result<i64> {
        Ok(self
            .task_queue
            .complete_bulk(event_ids, claim_token)
            .await?)
    }

    pub async fn fail_bulk(
        &self,
        event_ids: &[(String, String)],
        claim_token: &str,
    ) -> Result<Vec<(String, TaskStatus)>> {
        Ok(self
            .task_queue
            .fail_bulk_with_errors(
                event_ids,
                claim_token,
                true,
                CONNECTOR_EVENT_RETRY_DELAY_SECONDS,
            )
            .await?)
    }

    pub async fn dead_letter_pending(
        &self,
        event_ids: &[String],
        reason: &str,
    ) -> Result<Vec<String>> {
        self.task_queue.dead_letter_pending(event_ids, reason).await
    }
}

fn content_id(event: &ConnectorEvent) -> Option<&str> {
    match event {
        ConnectorEvent::DocumentCreated { content_id, .. }
        | ConnectorEvent::DocumentUpdated { content_id, .. } => Some(content_id),
        ConnectorEvent::DocumentDeleted { .. }
        | ConnectorEvent::GroupMembershipSync { .. }
        | ConnectorEvent::PersonSync { .. }
        | ConnectorEvent::PersonDeleted { .. } => None,
    }
}

fn person_concurrency_key(event: &ConnectorEvent) -> Option<String> {
    match event {
        ConnectorEvent::PersonSync {
            source_id, person, ..
        } => Some(format!(
            "{}:{}",
            source_id,
            person.email.trim().to_lowercase()
        )),
        ConnectorEvent::PersonDeleted {
            source_id, email, ..
        } => Some(format!("{}:{}", source_id, email.trim().to_lowercase())),
        _ => None,
    }
}

fn serialized_weight(payload: &Value) -> i64 {
    serde_json::to_vec(payload)
        .map(|bytes| bytes.len() as i64)
        .unwrap_or(0)
}

fn task_to_item(task: Task) -> Result<ConnectorEventQueueItem> {
    let task_id = task.id.clone();
    if task.task_type != CONNECTOR_EVENT_TASK_TYPE {
        bail!("task {}: unexpected task type {}", task_id, task.task_type);
    }
    if task.payload_version != CONNECTOR_EVENT_PAYLOAD_VERSION {
        bail!(
            "task {}: unsupported connector payload version {}",
            task_id,
            task.payload_version
        );
    }
    let payload = task.payload.clone();
    let event: ConnectorEvent = serde_json::from_value(payload.clone())
        .with_context(|| format!("task {}: malformed connector event payload", task_id))?;
    let status = match task.status {
        TaskStatus::Pending => EventStatus::Pending,
        TaskStatus::Running => EventStatus::Processing,
        TaskStatus::Completed => EventStatus::Completed,
        TaskStatus::DeadLetter => EventStatus::DeadLetter,
    };
    Ok(ConnectorEventQueueItem {
        id: task.id,
        source_id: event.source_id().to_string(),
        sync_run_id: event.sync_run_id().to_string(),
        event_type: event_type(&event).to_string(),
        payload,
        event,
        status,
        retry_count: task.attempt_count,
        max_retries: task.max_attempts,
        created_at: task.created_at,
        processed_at: task.completed_at,
        error_message: task.last_error,
        claim_token: task.claim_token,
        weight: task.weight,
    })
}

#[derive(Debug)]
pub struct QueueSummaryEntry {
    pub sync_type: Option<SyncType>,
    pub status: EventStatus,
    pub count: i64,
    pub oldest: Option<chrono::DateTime<chrono::Utc>>,
    pub size_bytes: i64,
    pub has_completed_sync: bool,
}

#[derive(Debug)]
pub struct QueueSummary {
    pub entries: Vec<QueueSummaryEntry>,
}

#[derive(Debug, Default, serde::Serialize)]
pub struct QueueStats {
    pub pending: i64,
    pub processing: i64,
    pub completed: i64,
    /// Retained as a zero-valued field for JSON consumers of the old stats
    /// shape. Generic tasks do not have a failed state.
    pub failed: i64,
    pub dead_letter: i64,
}

#[derive(Debug, serde::Serialize)]
pub struct CleanupResult {
    pub completed_deleted: u64,
    pub dead_letter_deleted: u64,
}

pub type EventQueue = ConnectorEventQueue;

#[allow(dead_code)]
fn _event_type_is_person(event: &ConnectorEvent) -> bool {
    is_person_event_type(event_type(event))
}
