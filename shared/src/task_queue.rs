use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};
use time::OffsetDateTime;

use crate::utils::generate_ulid;

/// Lifecycle state of a task. There is no transient `failed` state: a
/// retryable failure returns the task to `pending` with a future
/// `available_at`, and exhausted or non-retryable failures become
/// `dead_letter`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Pending,
    Running,
    Completed,
    DeadLetter,
}

impl TaskStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            TaskStatus::Pending => "pending",
            TaskStatus::Running => "running",
            TaskStatus::Completed => "completed",
            TaskStatus::DeadLetter => "dead_letter",
        }
    }
}

impl std::fmt::Display for TaskStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for TaskStatus {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        match s {
            "pending" => Ok(TaskStatus::Pending),
            "running" => Ok(TaskStatus::Running),
            "completed" => Ok(TaskStatus::Completed),
            "dead_letter" => Ok(TaskStatus::DeadLetter),
            _ => Err(anyhow::anyhow!("invalid task status: {}", s)),
        }
    }
}

/// A full row from the `tasks` table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskRow {
    pub id: String,
    pub task_type: String,
    pub payload: serde_json::Value,
    pub payload_version: i32,
    pub status: TaskStatus,
    pub priority: i32,
    pub available_at: OffsetDateTime,
    pub weight: i64,
    pub concurrency_key: Option<String>,
    pub attempt_count: i32,
    pub max_attempts: i32,
    pub last_error: Option<String>,
    pub claim_token: Option<String>,
    pub claimed_by: Option<String>,
    pub lease_expires_at: Option<OffsetDateTime>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
    pub last_started_at: Option<OffsetDateTime>,
    pub completed_at: Option<OffsetDateTime>,
}

impl sqlx::FromRow<'_, sqlx::postgres::PgRow> for TaskRow {
    fn from_row(row: &sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row;

        let status_str: String = row.try_get("status")?;
        let status = status_str
            .parse::<TaskStatus>()
            .map_err(|e| sqlx::Error::ColumnDecode {
                index: "status".to_string(),
                source: e.into(),
            })?;

        Ok(TaskRow {
            id: row.try_get("id")?,
            task_type: row.try_get("task_type")?,
            payload: row.try_get("payload")?,
            payload_version: row.try_get("payload_version")?,
            status,
            priority: row.try_get("priority")?,
            available_at: row.try_get("available_at")?,
            weight: row.try_get("weight")?,
            concurrency_key: row.try_get("concurrency_key")?,
            attempt_count: row.try_get("attempt_count")?,
            max_attempts: row.try_get("max_attempts")?,
            last_error: row.try_get("last_error")?,
            claim_token: row.try_get("claim_token")?,
            claimed_by: row.try_get("claimed_by")?,
            lease_expires_at: row.try_get("lease_expires_at")?,
            created_at: row.try_get("created_at")?,
            updated_at: row.try_get("updated_at")?,
            last_started_at: row.try_get("last_started_at")?,
            completed_at: row.try_get("completed_at")?,
        })
    }
}

/// A task to enqueue. `id` defaults to a fresh ULID; producers that need
/// idempotent retries must set it explicitly and reuse it.
#[derive(Debug, Clone)]
pub struct NewTask {
    pub id: String,
    pub task_type: String,
    pub payload: serde_json::Value,
    pub payload_version: i32,
    pub priority: i32,
    pub available_at: OffsetDateTime,
    pub weight: i64,
    pub concurrency_key: Option<String>,
    pub max_attempts: i32,
}

impl NewTask {
    pub fn new(task_type: impl Into<String>, payload: serde_json::Value) -> Self {
        Self {
            id: generate_ulid(),
            task_type: task_type.into(),
            payload,
            payload_version: 1,
            priority: 0,
            available_at: OffsetDateTime::now_utc(),
            weight: 1,
            concurrency_key: None,
            max_attempts: 3,
        }
    }
}

/// Claim selection and policy options. A claim selects eligible tasks either
/// by `task_type` or by explicit `candidate_ids` chosen by the caller (e.g.
/// in a transaction that already applied workload-specific filtering).
#[derive(Debug, Clone)]
pub struct ClaimOptions {
    /// Claim only these task ids (already eligible per the caller's own
    /// selection). When `None`, claim by `task_type`.
    pub candidate_ids: Option<Vec<String>>,
    /// Maximum number of tasks to claim.
    pub limit: i32,
    /// Cap on cumulative batch weight; the first eligible task is always
    /// admitted even if it alone exceeds the cap.
    pub max_weight: Option<i64>,
    /// Cap on concurrently running tasks of this type. Enforced atomically
    /// per task type.
    pub max_concurrency: Option<i32>,
    /// Lease duration in seconds granted by this claim.
    pub lease_seconds: i32,
}

impl Default for ClaimOptions {
    fn default() -> Self {
        Self {
            candidate_ids: None,
            limit: 1,
            max_weight: None,
            max_concurrency: None,
            lease_seconds: 300,
        }
    }
}

/// A successful claim: a fresh fencing token plus the tasks now leased to the
/// caller. Terminal writes (complete/fail) must be fenced with this token.
#[derive(Debug, Clone)]
pub struct TaskClaim {
    pub claim_token: String,
    pub tasks: Vec<TaskRow>,
}

/// Status statistics grouped by (task_type, status).
#[derive(Debug, Clone, Serialize)]
pub struct TaskStats {
    pub task_type: String,
    pub status: TaskStatus,
    pub count: i64,
}

/// Thin, strongly typed facade over the canonical task queue PostgreSQL
/// functions created by migration 112. All queue semantics live in SQL so the
/// Rust and Python facades can never drift apart.
#[derive(Clone)]
pub struct TaskQueue {
    pool: PgPool,
}

impl TaskQueue {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Enqueue tasks, returning only the rows that were actually inserted.
    /// Re-enqueueing an existing id is a no-op, which is what makes producer
    /// retries idempotent.
    pub async fn enqueue_bulk(&self, tasks: &[NewTask]) -> Result<Vec<TaskRow>> {
        if tasks.is_empty() {
            return Ok(Vec::new());
        }
        for task in tasks {
            validate_new_task(task)?;
        }

        let ids: Vec<String> = tasks.iter().map(|t| t.id.clone()).collect();
        let task_types: Vec<String> = tasks.iter().map(|t| t.task_type.clone()).collect();
        let payloads: Vec<serde_json::Value> = tasks.iter().map(|t| t.payload.clone()).collect();
        let payload_versions: Vec<i32> = tasks.iter().map(|t| t.payload_version).collect();
        let priorities: Vec<i32> = tasks.iter().map(|t| t.priority).collect();
        let available_at_ms: Vec<i64> = tasks.iter().map(|t| to_epoch_ms(t.available_at)).collect();
        let weights: Vec<i64> = tasks.iter().map(|t| t.weight).collect();
        let concurrency_keys: Vec<Option<String>> =
            tasks.iter().map(|t| t.concurrency_key.clone()).collect();
        let max_attempts: Vec<i32> = tasks.iter().map(|t| t.max_attempts).collect();

        let rows = sqlx::query_as::<_, TaskRow>(
            "SELECT * FROM task_enqueue_bulk($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        )
        .bind(&ids)
        .bind(&task_types)
        .bind(&payloads)
        .bind(&payload_versions)
        .bind(&priorities)
        .bind(&available_at_ms)
        .bind(&weights)
        .bind(&concurrency_keys)
        .bind(&max_attempts)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows)
    }

    /// Convenience wrapper around [`TaskQueue::enqueue_bulk`]. Re-enqueueing
    /// an existing id is idempotent and returns the already-stored task.
    pub async fn enqueue(&self, task: NewTask) -> Result<TaskRow> {
        let created = self.enqueue_bulk(std::slice::from_ref(&task)).await?;
        if let Some(row) = created.into_iter().next() {
            return Ok(row);
        }
        let existing = sqlx::query_as::<_, TaskRow>("SELECT * FROM tasks WHERE id = $1")
            .bind(&task.id)
            .fetch_optional(&self.pool)
            .await?;
        existing.ok_or_else(|| anyhow::anyhow!("task {} was not enqueued", task.id))
    }

    /// Atomically claim a batch of tasks. Pass a pool, connection, or
    /// transaction as `executor`; the transaction form lets callers select
    /// candidate ids with workload-specific SQL and claim them in the same
    /// transaction. Generates the batch claim token internally.
    pub async fn claim_bulk<'e, E>(
        &self,
        executor: E,
        task_type: &str,
        claimed_by: &str,
        options: &ClaimOptions,
    ) -> Result<TaskClaim>
    where
        E: sqlx::Executor<'e, Database = sqlx::Postgres>,
    {
        if task_type.trim().is_empty() {
            bail!("claim task_type must not be empty");
        }
        if claimed_by.trim().is_empty() {
            bail!("claim claimed_by must not be empty");
        }
        validate_claim_options(options)?;

        let claim_token = generate_ulid();
        let rows = sqlx::query_as::<_, TaskRow>(
            "SELECT * FROM task_claim_bulk($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(task_type)
        .bind(&options.candidate_ids)
        .bind(options.limit)
        .bind(options.max_weight)
        .bind(options.max_concurrency)
        .bind(options.lease_seconds)
        .bind(&claim_token)
        .bind(claimed_by)
        .fetch_all(executor)
        .await?;

        Ok(TaskClaim {
            claim_token,
            tasks: rows,
        })
    }

    /// Renew a running task's lease. Returns `false` when the task is no
    /// longer running under this token or its lease has already expired; the
    /// worker must then stop processing the task.
    pub async fn heartbeat(
        &self,
        task_id: &str,
        claim_token: &str,
        lease_seconds: i32,
    ) -> Result<bool> {
        if task_id.len() != 26 {
            bail!("heartbeat task id must be a 26-char ULID");
        }
        if claim_token.len() != 26 {
            bail!("heartbeat claim_token must be a 26-char ULID");
        }
        if lease_seconds < 1 {
            bail!("heartbeat lease_seconds must be >= 1");
        }
        let renewed = sqlx::query_scalar::<_, bool>("SELECT task_heartbeat($1, $2, $3)")
            .bind(task_id)
            .bind(claim_token)
            .bind(lease_seconds)
            .fetch_one(&self.pool)
            .await?;
        Ok(renewed)
    }

    /// Mark claimed tasks completed. Fenced by the batch claim token and an
    /// unexpired lease. Returns the number of tasks completed.
    pub async fn complete_bulk(&self, task_ids: &[String], claim_token: &str) -> Result<i64> {
        if claim_token.len() != 26 {
            bail!("complete claim_token must be a 26-char ULID");
        }
        if task_ids.is_empty() {
            return Ok(0);
        }
        let completed = sqlx::query_scalar::<_, i64>("SELECT task_complete_bulk($1, $2)")
            .bind(task_ids)
            .bind(claim_token)
            .fetch_one(&self.pool)
            .await?;
        Ok(completed)
    }

    /// Fail claimed tasks. Retryable tasks with attempt budget left return to
    /// `pending` and become claimable again after `retry_delay_seconds`;
    /// everything else becomes `dead_letter`. Returns the resulting status
    /// per task id.
    pub async fn fail_bulk(
        &self,
        task_ids: &[String],
        claim_token: &str,
        error: &str,
        retryable: bool,
        retry_delay_seconds: i32,
    ) -> Result<Vec<(String, TaskStatus)>> {
        if claim_token.len() != 26 {
            bail!("fail claim_token must be a 26-char ULID");
        }
        if retry_delay_seconds < 0 {
            bail!("fail retry_delay_seconds must be >= 0");
        }
        if task_ids.is_empty() {
            return Ok(Vec::new());
        }
        let rows = sqlx::query("SELECT * FROM task_fail_bulk($1, $2, $3, $4, $5)")
            .bind(task_ids)
            .bind(claim_token)
            .bind(error)
            .bind(retryable)
            .bind(retry_delay_seconds)
            .fetch_all(&self.pool)
            .await?;

        let mut result = Vec::with_capacity(rows.len());
        for row in rows {
            let task_id: String = row.get("task_id");
            let status: TaskStatus = row.get::<String, _>("result_status").parse()?;
            result.push((task_id, status));
        }
        Ok(result)
    }

    /// Recover tasks whose lease expired while running: retryable tasks are
    /// requeued as pending, exhausted tasks become dead_letter. Returns every
    /// recovered row so adapters that mirror queue state into domain tables
    /// can synchronize.
    pub async fn recover_expired(&self) -> Result<Vec<TaskRow>> {
        let rows = sqlx::query_as::<_, TaskRow>("SELECT * FROM task_recover_expired()")
            .fetch_all(&self.pool)
            .await?;
        Ok(rows)
    }

    /// Status statistics grouped by (task_type, status). Pass `None` for all
    /// task types.
    pub async fn stats(&self, task_type: Option<&str>) -> Result<Vec<TaskStats>> {
        let rows = sqlx::query("SELECT * FROM task_stats($1)")
            .bind(task_type)
            .fetch_all(&self.pool)
            .await?;

        let mut stats = Vec::with_capacity(rows.len());
        for row in rows {
            stats.push(TaskStats {
                task_type: row.get("task_type"),
                status: row.get::<String, _>("status").parse()?,
                count: row.get("count"),
            });
        }
        Ok(stats)
    }

    /// Delete terminal (completed/dead_letter) tasks completed before the
    /// cutoff. Returns the number of deleted rows.
    pub async fn cleanup(&self, before: OffsetDateTime) -> Result<i64> {
        let deleted = sqlx::query_scalar::<_, i64>("SELECT task_cleanup($1)")
            .bind(before)
            .fetch_one(&self.pool)
            .await?;
        Ok(deleted)
    }
}

fn to_epoch_ms(odt: OffsetDateTime) -> i64 {
    odt.unix_timestamp() * 1000 + i64::from(odt.millisecond())
}

fn validate_new_task(task: &NewTask) -> Result<()> {
    if task.id.len() != 26 {
        bail!("task id must be a 26-char ULID, got {:?}", task.id);
    }
    if task.task_type.trim().is_empty() {
        bail!("task_type must not be empty");
    }
    if !task.payload.is_object() {
        bail!("task payload must be a JSON object");
    }
    if task.payload_version < 1 {
        bail!("payload_version must be >= 1");
    }
    if task.weight < 0 {
        bail!("task weight must be >= 0");
    }
    if task.max_attempts < 1 {
        bail!("max_attempts must be >= 1");
    }
    Ok(())
}

fn validate_claim_options(options: &ClaimOptions) -> Result<()> {
    if options.limit < 1 {
        bail!("claim limit must be >= 1");
    }
    if options.max_weight.is_some_and(|w| w < 0) {
        bail!("claim max_weight must be >= 0");
    }
    if options.max_concurrency.is_some_and(|c| c < 1) {
        bail!("claim max_concurrency must be >= 1");
    }
    if options.lease_seconds < 1 {
        bail!("claim lease_seconds must be >= 1");
    }
    if let Some(ids) = &options.candidate_ids {
        for id in ids {
            if id.len() != 26 {
                bail!("candidate task id must be a 26-char ULID, got {:?}", id);
            }
        }
    }
    Ok(())
}
