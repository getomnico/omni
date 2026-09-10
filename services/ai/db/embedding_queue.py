"""Embedding workload adapter over the generic task queue."""

from dataclasses import dataclass
from datetime import UTC, datetime, timedelta
from enum import StrEnum

from asyncpg import Connection, Pool

from .connection import get_db_pool
from .task_queue import (
    ClaimOptions,
    EnqueueTaskRequest,
    Task,
    TaskClaim,
    TaskQueueRepository,
    TaskStatus,
)

DOCUMENT_EMBEDDING_TASK_TYPE = "document_embedding"
DOCUMENT_EMBEDDING_PAYLOAD_VERSION = 1
DOCUMENT_EMBEDDING_MAX_ATTEMPTS = 5


class QueueStatus(StrEnum):
    PENDING = "pending"
    PROCESSING = "running"
    COMPLETED = "completed"
    FAILED = "dead_letter"


@dataclass
class EmbeddingQueueItem:
    """Validated document embedding task payload and generic task metadata."""

    id: str
    document_id: str
    status: QueueStatus
    error_message: str | None
    retry_count: int
    created_at: datetime
    updated_at: datetime
    processed_at: datetime | None
    claim_token: str | None

    @classmethod
    def from_task(cls, task: Task) -> "EmbeddingQueueItem":
        if task.task_type != DOCUMENT_EMBEDDING_TASK_TYPE:
            raise ValueError(f"unexpected embedding task type: {task.task_type!r}")
        if task.payload_version != DOCUMENT_EMBEDDING_PAYLOAD_VERSION:
            raise ValueError(
                f"unsupported embedding payload version: {task.payload_version}"
            )
        document_id = task.payload.get("document_id")
        if not isinstance(document_id, str) or not document_id:
            raise ValueError("embedding task payload requires document_id")
        status = {
            TaskStatus.PENDING: QueueStatus.PENDING,
            TaskStatus.RUNNING: QueueStatus.PROCESSING,
            TaskStatus.COMPLETED: QueueStatus.COMPLETED,
            TaskStatus.DEAD_LETTER: QueueStatus.FAILED,
        }[task.status]
        return cls(
            id=task.id,
            document_id=document_id,
            status=status,
            error_message=task.last_error,
            retry_count=task.attempt_count,
            created_at=task.created_at,
            updated_at=task.updated_at,
            processed_at=task.completed_at,
            claim_token=task.claim_token,
        )


class EmbeddingQueueRepository:
    """Provider-gated adapter for document embedding tasks."""

    def __init__(self, pool: Pool | None = None):
        self.pool = pool
        self.task_queue = TaskQueueRepository(pool)

    async def _get_pool(self) -> Pool:
        if self.pool:
            return self.pool
        return await get_db_pool()

    async def get_by_id(self, item_id: str) -> EmbeddingQueueItem | None:
        task = await self.task_queue.get(item_id)
        return EmbeddingQueueItem.from_task(task) if task else None

    async def get_status_counts(self) -> dict[str, int]:
        stats = await self.task_queue.stats(DOCUMENT_EMBEDDING_TASK_TYPE)
        return {str(stat.status): stat.count for stat in stats}

    async def get_pending_count(self, max_retries: int) -> int:
        pool = await self._get_pool()
        return int(
            await pool.fetchval(
                """
                SELECT COUNT(*) FROM tasks
                WHERE task_type = $1 AND status = 'pending'
                  AND attempt_count < LEAST(max_attempts, $2)
                """,
                DOCUMENT_EMBEDDING_TASK_TYPE,
                max_retries,
            )
        )

    async def claim_batch(self, limit: int, worker: str = "embedding-worker") -> TaskClaim:
        return await self.task_queue.claim(
            DOCUMENT_EMBEDDING_TASK_TYPE,
            worker,
            ClaimOptions(limit=limit, lease_seconds=900),
        )

    async def get_pending_items(
        self, limit: int, max_retries: int
    ) -> list[EmbeddingQueueItem]:
        claim = await self.claim_batch(limit)
        return [EmbeddingQueueItem.from_task(task) for task in claim.tasks]

    async def mark_completed_with_token(
        self,
        item_ids: list[str],
        claim_token: str,
        *,
        connection: Connection | None = None,
    ) -> None:
        if not item_ids:
            return
        if connection is None:
            completed = await self.task_queue.complete(item_ids, claim_token)
        else:
            completed = await connection.fetchval(
                "SELECT task_complete_bulk($1, $2)", item_ids, claim_token
            )
        if int(completed) != len(item_ids):
            raise RuntimeError("embedding task completion was fenced")

    async def mark_completed(self, item_ids: list[str]) -> None:
        if not item_ids:
            return
        token = await self._token_for(item_ids)
        await self.mark_completed_with_token(item_ids, token)

    async def mark_failed_with_token(
        self,
        item_ids: list[str],
        claim_token: str,
        error: str,
        *,
        retryable: bool = True,
    ) -> None:
        if item_ids:
            results = await self.task_queue.fail(
                item_ids,
                claim_token,
                error,
                retryable=retryable,
                retry_delay_seconds=0,
            )
            if len(results) != len(item_ids):
                raise RuntimeError("embedding task failure was fenced")

    async def mark_failed(self, item_ids: list[str], error: str) -> None:
        if not item_ids:
            return
        token = await self._token_for(item_ids)
        await self.mark_failed_with_token(item_ids, token, error)

    async def heartbeat(self, task_id: str, claim_token: str, lease_seconds: int) -> bool:
        return await self.task_queue.heartbeat(task_id, claim_token, lease_seconds)

    async def recover_stale_processing_items(self, timeout_seconds: int) -> int:
        if timeout_seconds < 1:
            raise ValueError("recovery timeout must be >= 1")
        pool = await self._get_pool()
        rows = await pool.fetch(
            "SELECT * FROM task_recover_stale_for_type($1, $2)",
            DOCUMENT_EMBEDDING_TASK_TYPE,
            timeout_seconds,
        )
        return len(rows)

    async def cleanup_completed(self, days_old: int) -> int:
        pool = await self._get_pool()
        cutoff = datetime.now(UTC) - timedelta(days=days_old)
        return int(
            await pool.fetchval(
                "SELECT task_cleanup_for_type($1, $2)",
                DOCUMENT_EMBEDDING_TASK_TYPE,
                cutoff,
            )
        )

    async def cleanup_failed(self, days_old: int) -> int:
        return await self.cleanup_completed(days_old)

    async def _token_for(self, item_ids: list[str]) -> str:
        pool = await self._get_pool()
        token = await pool.fetchval(
            "SELECT claim_token FROM tasks WHERE id = ANY($1) AND status = 'running' LIMIT 1",
            item_ids,
        )
        if not isinstance(token, str):
            raise RuntimeError("embedding tasks are not currently claimed")
        return token


def embedding_task(document_id: str) -> EnqueueTaskRequest:
    return EnqueueTaskRequest(
        task_type=DOCUMENT_EMBEDDING_TASK_TYPE,
        payload={"document_id": document_id},
        payload_version=DOCUMENT_EMBEDDING_PAYLOAD_VERSION,
        deduplication_key=document_id,
        max_attempts=DOCUMENT_EMBEDDING_MAX_ATTEMPTS,
    )
