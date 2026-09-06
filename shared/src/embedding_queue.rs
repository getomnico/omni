//! Embedding workload adapter for the generic task queue.
//!
//! Document identifiers are encoded in the versioned task payload. This module
//! owns provider gating and the current-model eligibility query, while lease,
//! retry, fencing, and deduplication remain generic task-queue behavior.

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use time::Duration;

use crate::{
    db::repositories::EmbeddingProviderRepository,
    task_queue::{ClaimOptions, EnqueueTaskRequest, Task, TaskClaim, TaskQueue, TaskStatus},
};

pub const DOCUMENT_EMBEDDING_TASK_TYPE: &str = "document_embedding";
pub const DOCUMENT_EMBEDDING_PAYLOAD_VERSION: i32 = 1;
pub const DOCUMENT_EMBEDDING_MAX_ATTEMPTS: i32 = 5;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EmbeddingQueueStatus {
    Pending,
    Processing,
    Completed,
    Failed,
}

impl std::fmt::Display for EmbeddingQueueStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pending => write!(f, "pending"),
            Self::Processing => write!(f, "processing"),
            Self::Completed => write!(f, "completed"),
            Self::Failed => write!(f, "failed"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingQueueItem {
    pub id: String,
    pub document_id: String,
    pub status: EmbeddingQueueStatus,
    pub retry_count: i32,
    pub error_message: Option<String>,
    pub created_at: sqlx::types::time::OffsetDateTime,
    pub updated_at: sqlx::types::time::OffsetDateTime,
    pub processed_at: Option<sqlx::types::time::OffsetDateTime>,
    /// Fencing token for the claim that produced this adapter item.
    pub claim_token: Option<String>,
}

#[derive(Clone)]
pub struct EmbeddingQueue {
    pool: PgPool,
    provider_repo: EmbeddingProviderRepository,
    task_queue: TaskQueue,
}

impl EmbeddingQueue {
    pub fn new(pool: PgPool) -> Self {
        let provider_repo = EmbeddingProviderRepository::new(&pool);
        let task_queue = TaskQueue::new(pool.clone());
        Self {
            pool,
            provider_repo,
            task_queue,
        }
    }

    pub async fn enqueue(&self, document_id: String) -> Result<Option<String>> {
        let ids = self.enqueue_batch(vec![document_id]).await?;
        Ok(ids.into_iter().next())
    }

    pub async fn enqueue_batch(&self, document_ids: Vec<String>) -> Result<Vec<String>> {
        if document_ids.is_empty() || !self.provider_repo.has_active_provider().await? {
            return Ok(Vec::new());
        }

        let tasks: Vec<EnqueueTaskRequest> = document_ids
            .into_iter()
            .map(|document_id| embedding_task(document_id))
            .collect();
        let inserted = self.task_queue.enqueue_bulk(&tasks).await?;
        Ok(inserted.into_iter().map(|task| task.id).collect())
    }

    pub async fn enqueue_batch_missing_current_embeddings(
        &self,
        document_ids: Vec<String>,
    ) -> Result<Vec<String>> {
        if document_ids.is_empty() || !self.provider_repo.has_active_provider().await? {
            return Ok(Vec::new());
        }

        let candidates: Vec<String> = sqlx::query_scalar(
            r#"
            WITH active_provider AS (
                SELECT config->>'model' AS model_name
                FROM embedding_providers
                WHERE is_current = TRUE AND is_deleted = FALSE
                LIMIT 1
            )
            SELECT DISTINCT d.id
            FROM UNNEST($1::text[]) AS input(document_id)
            JOIN documents d ON d.id = input.document_id
            CROSS JOIN active_provider provider
            WHERE NOT EXISTS (
                SELECT 1 FROM tasks q
                WHERE q.task_type = $2
                  AND q.deduplication_key = d.id
                  AND q.status IN ('pending', 'running')
            )
            AND NOT EXISTS (
                SELECT 1 FROM embeddings e
                WHERE e.document_id = d.id
                  AND e.model_name = provider.model_name
            )
            ORDER BY d.id
            "#,
        )
        .bind(&document_ids)
        .bind(DOCUMENT_EMBEDDING_TASK_TYPE)
        .fetch_all(&self.pool)
        .await?;

        self.enqueue_batch(candidates).await
    }

    /// Claim embedding tasks with a fencing token. The old queue's five retry
    /// attempts are represented by each task's generic max_attempts field.
    pub async fn claim_batch(&self, batch_size: i32, worker: &str) -> Result<TaskClaim> {
        self.task_queue
            .claim_bulk(
                &self.pool,
                DOCUMENT_EMBEDDING_TASK_TYPE,
                worker,
                &ClaimOptions {
                    limit: batch_size,
                    lease_seconds: 900,
                    ..Default::default()
                },
            )
            .await
    }

    /// Compatibility-shaped method for callers that only need claimed items.
    pub async fn dequeue_batch(&self, batch_size: i32) -> Result<Vec<EmbeddingQueueItem>> {
        let claim = self.claim_batch(batch_size, "embedding-adapter").await?;
        claim.tasks.into_iter().map(task_to_item).collect()
    }

    pub async fn get_by_id(&self, id: &str) -> Result<Option<EmbeddingQueueItem>> {
        let task = self.task_queue.get(id).await?;
        task.map(task_to_item).transpose()
    }

    pub async fn get_queue_stats(&self) -> Result<QueueStats> {
        let stats = self
            .task_queue
            .stats(Some(DOCUMENT_EMBEDDING_TASK_TYPE))
            .await?;
        let mut result = QueueStats::default();
        for stat in stats {
            match stat.status {
                TaskStatus::Pending => result.pending = stat.count,
                TaskStatus::Running => result.processing = stat.count,
                TaskStatus::Completed => result.completed = stat.count,
                TaskStatus::DeadLetter => result.failed = stat.count,
            }
        }
        Ok(result)
    }

    pub async fn mark_completed(&self, ids: &[String]) -> Result<()> {
        let token = self.claim_token_for(ids).await?;
        self.mark_completed_with_token(ids, &token).await
    }

    pub async fn mark_completed_with_token(&self, ids: &[String], token: &str) -> Result<()> {
        let completed = self.task_queue.complete_bulk(ids, token).await?;
        if completed != ids.len() as i64 {
            bail!(
                "embedding task completion was fenced for {} tasks",
                ids.len() - completed as usize
            );
        }
        Ok(())
    }

    pub async fn mark_failed(&self, id: &str, error: &str) -> Result<()> {
        self.mark_failed_batch(std::slice::from_ref(&id.to_string()), error)
            .await
    }

    pub async fn mark_failed_batch(&self, ids: &[String], error: &str) -> Result<()> {
        let token = self.claim_token_for(ids).await?;
        self.mark_failed_batch_with_token(ids, &token, error).await
    }

    pub async fn mark_failed_batch_with_token(
        &self,
        ids: &[String],
        token: &str,
        error: &str,
    ) -> Result<()> {
        let failed = self
            .task_queue
            .fail_bulk(ids, token, error, true, 0)
            .await?;
        if failed.len() != ids.len() {
            bail!(
                "embedding task failure was fenced for {} tasks",
                ids.len() - failed.len()
            );
        }
        Ok(())
    }

    pub async fn recover_stale_processing_items(&self, timeout_seconds: i32) -> Result<i64> {
        if timeout_seconds < 1 {
            bail!("recovery timeout must be >= 1");
        }
        let rows = sqlx::query("SELECT * FROM task_recover_stale_for_type($1, $2)")
            .bind(DOCUMENT_EMBEDDING_TASK_TYPE)
            .bind(timeout_seconds)
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.len() as i64)
    }

    pub async fn cleanup_completed(&self, days_old: i32) -> Result<i64> {
        let cutoff =
            sqlx::types::time::OffsetDateTime::now_utc() - Duration::days(i64::from(days_old));
        sqlx::query_scalar("SELECT task_cleanup_for_type($1, $2)")
            .bind(DOCUMENT_EMBEDDING_TASK_TYPE)
            .bind(cutoff)
            .fetch_one(&self.pool)
            .await
            .map_err(Into::into)
    }

    pub async fn cleanup_failed(&self, days_old: i32) -> Result<i64> {
        self.cleanup_completed(days_old).await
    }

    async fn claim_token_for(&self, ids: &[String]) -> Result<String> {
        let token: Option<String> = sqlx::query_scalar(
            "SELECT claim_token FROM tasks WHERE id = ANY($1) AND status = 'running' LIMIT 1",
        )
        .bind(ids)
        .fetch_optional(&self.pool)
        .await?;
        token.ok_or_else(|| anyhow::anyhow!("embedding tasks are not currently claimed"))
    }
}

fn embedding_task(document_id: String) -> EnqueueTaskRequest {
    let mut task = EnqueueTaskRequest::new(
        DOCUMENT_EMBEDDING_TASK_TYPE,
        serde_json::json!({ "document_id": document_id }),
    );
    task.payload_version = DOCUMENT_EMBEDDING_PAYLOAD_VERSION;
    task.deduplication_key = Some(document_id);
    task.max_attempts = DOCUMENT_EMBEDDING_MAX_ATTEMPTS;
    task
}

fn task_to_item(task: Task) -> Result<EmbeddingQueueItem> {
    if task.task_type != DOCUMENT_EMBEDDING_TASK_TYPE {
        bail!(
            "unexpected task type for embedding item: {}",
            task.task_type
        );
    }
    if task.payload_version != DOCUMENT_EMBEDDING_PAYLOAD_VERSION {
        bail!(
            "unsupported embedding payload version: {}",
            task.payload_version
        );
    }
    let document_id = task
        .payload
        .get("document_id")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow::anyhow!("embedding task payload requires document_id"))?
        .to_string();
    let status = match task.status {
        TaskStatus::Pending => EmbeddingQueueStatus::Pending,
        TaskStatus::Running => EmbeddingQueueStatus::Processing,
        TaskStatus::Completed => EmbeddingQueueStatus::Completed,
        TaskStatus::DeadLetter => EmbeddingQueueStatus::Failed,
    };
    Ok(EmbeddingQueueItem {
        id: task.id,
        document_id,
        status,
        retry_count: task.attempt_count,
        error_message: task.last_error,
        created_at: task.created_at,
        updated_at: task.updated_at,
        processed_at: task.completed_at,
        claim_token: task.claim_token,
    })
}

#[derive(Debug, Default, Serialize)]
pub struct QueueStats {
    pub pending: i64,
    pub processing: i64,
    pub completed: i64,
    pub failed: i64,
}
