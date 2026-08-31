"""Repository for the generic PostgreSQL task queue.

This is the Python counterpart of the Rust `shared::task_queue` module. Both
languages are thin adapters over the canonical `task_*` PostgreSQL functions
created by migration 112, so the lifecycle state machine lives in exactly one
place (the database) and the two facades cannot drift apart.

Contract notes:

- Delivery is at-least-once: a task can be claimed again after its lease
  expires, so consumers must be idempotent around database writes and
  external effects.
- A consumer that loses its lease (``heartbeat`` returns False, or a terminal
  write affects no rows) must stop processing the task.
- Claim result order is not guaranteed by ``UPDATE ... RETURNING``; consumers
  that need order must sort the returned tasks.
- Enqueue idempotency is per task id: producers retrying must reuse the same
  ``EnqueueTaskRequest.id``. There is no generic deduplication key; logical work
  coalescing is workload policy (enforced with task-specific indexes in the
  workload migrations).
- A non-null ``concurrency_key`` only serializes execution: multiple tasks may
  queue for the same key, but they run one at a time in oldest-task order.
"""

import json
import logging
from dataclasses import dataclass
from datetime import UTC, datetime
from enum import StrEnum

from asyncpg import Connection, Pool
from ulid import ULID

from .connection import get_db_pool

logger = logging.getLogger(__name__)

JsonValue = str | int | float | bool | None | list["JsonValue"] | dict[str, "JsonValue"]


class TaskStatus(StrEnum):
    PENDING = "pending"
    RUNNING = "running"
    COMPLETED = "completed"
    DEAD_LETTER = "dead_letter"


@dataclass
class Task:
    """A row from the tasks table."""

    id: str
    task_type: str
    payload: dict[str, JsonValue]
    payload_version: int
    status: TaskStatus
    priority: int
    available_at: datetime
    weight: int
    concurrency_key: str | None
    attempt_count: int
    max_attempts: int
    last_error: str | None
    claim_token: str | None
    claimed_by: str | None
    lease_expires_at: datetime | None
    created_at: datetime
    updated_at: datetime
    last_started_at: datetime | None
    completed_at: datetime | None

    @classmethod
    def from_row(cls, row) -> "Task":
        data = dict(row)
        data["status"] = TaskStatus(data["status"])
        if isinstance(data["payload"], str):
            data["payload"] = json.loads(data["payload"])
        return cls(**data)


@dataclass
class EnqueueTaskRequest:
    """A task to enqueue. ``id`` defaults to a fresh ULID; producers that
    need idempotent retries must set it explicitly and reuse it."""

    task_type: str
    payload: dict[str, JsonValue]
    id: str | None = None
    payload_version: int = 1
    priority: int = 0
    available_at: datetime | None = None
    weight: int = 1
    concurrency_key: str | None = None
    max_attempts: int = 3


@dataclass
class ClaimOptions:
    """Claim selection and policy options."""

    candidate_ids: list[str] | None = None
    limit: int = 1
    max_weight: int | None = None
    max_concurrency: int | None = None
    lease_seconds: int = 300


@dataclass
class TaskClaim:
    """A successful claim: a fresh fencing token plus the leased tasks.
    Terminal writes (complete/fail) must be fenced with ``claim_token``."""

    claim_token: str
    tasks: list[Task]


@dataclass
class TaskStats:
    """Status statistics grouped by (task_type, status)."""

    task_type: str
    status: TaskStatus
    count: int


def _to_epoch_ms(dt: datetime | None) -> int:
    if dt is None:
        return int(datetime.now(UTC).timestamp() * 1000)
    if dt.tzinfo is None:
        dt = dt.replace(tzinfo=UTC)
    return int(dt.timestamp() * 1000)


class TaskQueueRepository:
    """Thin facade over the canonical task queue PostgreSQL functions."""

    def __init__(self, pool: Pool | None = None):
        self.pool = pool

    async def _get_pool(self) -> Pool:
        if self.pool:
            return self.pool
        return await get_db_pool()

    async def get(self, task_id: str) -> Task | None:
        """Fetch one task by id."""
        pool = await self._get_pool()
        row = await pool.fetchrow("SELECT * FROM tasks WHERE id = $1", task_id)
        return Task.from_row(row) if row else None

    async def enqueue(self, task: EnqueueTaskRequest) -> Task:
        """Enqueue one task. Re-enqueueing an existing id is idempotent and
        returns the already-stored task."""
        created = await self.enqueue_bulk([task])
        if created:
            return created[0]
        if task.id is None:
            raise RuntimeError("task was not enqueued")
        existing = await self.get(task.id)
        if existing is None:
            raise RuntimeError(f"task {task.id} was not enqueued")
        return existing

    async def enqueue_bulk(self, tasks: list[EnqueueTaskRequest]) -> list[Task]:
        """Enqueue tasks, returning only the rows actually inserted."""
        if not tasks:
            return []

        for task in tasks:
            self._validate_enqueue_task_request(task)

        ids = [task.id or str(ULID()) for task in tasks]
        task_types = [task.task_type for task in tasks]
        payloads = [json.dumps(task.payload) for task in tasks]
        payload_versions = [task.payload_version for task in tasks]
        priorities = [task.priority for task in tasks]
        available_at_ms = [_to_epoch_ms(task.available_at) for task in tasks]
        weights = [task.weight for task in tasks]
        concurrency_keys = [task.concurrency_key for task in tasks]
        max_attempts = [task.max_attempts for task in tasks]

        pool = await self._get_pool()
        rows = await pool.fetch(
            "SELECT * FROM task_enqueue_bulk($1, $2, $3, $4, $5, $6, $7, $8, $9)",
            ids,
            task_types,
            payloads,
            payload_versions,
            priorities,
            available_at_ms,
            weights,
            concurrency_keys,
            max_attempts,
        )
        return [Task.from_row(row) for row in rows]

    async def claim(
        self,
        task_type: str,
        claimed_by: str,
        options: ClaimOptions,
        *,
        connection: Connection | None = None,
    ) -> TaskClaim:
        """Atomically claim a batch of tasks.

        Pass ``connection`` to claim inside a transaction that already applied
        workload-specific candidate selection; the transaction makes the
        selection and the claim atomic.
        """
        if not task_type.strip():
            raise ValueError("task_type must not be empty")
        if not claimed_by.strip():
            raise ValueError("claimed_by must not be empty")
        if options.limit < 1:
            raise ValueError("claim limit must be >= 1")
        if options.max_weight is not None and options.max_weight < 0:
            raise ValueError("claim max_weight must be >= 0")
        if options.max_concurrency is not None and options.max_concurrency < 1:
            raise ValueError("claim max_concurrency must be >= 1")
        if options.lease_seconds < 1:
            raise ValueError("claim lease_seconds must be >= 1")
        if options.candidate_ids:
            for candidate_id in options.candidate_ids:
                if len(candidate_id) != 26:
                    raise ValueError(
                        f"candidate task id must be a 26-char ULID, got {candidate_id!r}"
                    )

        pool = connection or await self._get_pool()
        claim_token = str(ULID())
        rows = await pool.fetch(
            "SELECT * FROM task_claim_bulk($1, $2, $3, $4, $5, $6, $7, $8)",
            task_type,
            options.candidate_ids,
            options.limit,
            options.max_weight,
            options.max_concurrency,
            options.lease_seconds,
            claim_token,
            claimed_by,
        )
        return TaskClaim(claim_token=claim_token, tasks=[Task.from_row(r) for r in rows])

    async def heartbeat(self, task_id: str, claim_token: str, lease_seconds: int) -> bool:
        """Renew a running task's lease. Returns False when the task is no
        longer running under this token or its lease has expired; the worker
        must then stop processing it."""
        if len(task_id) != 26:
            raise ValueError(f"task id must be a 26-char ULID, got {task_id!r}")
        if len(claim_token) != 26:
            raise ValueError(f"claim_token must be a 26-char ULID, got {claim_token!r}")
        if lease_seconds < 1:
            raise ValueError("heartbeat lease_seconds must be >= 1")
        pool = await self._get_pool()
        renewed = await pool.fetchval(
            "SELECT task_heartbeat($1, $2, $3)", task_id, claim_token, lease_seconds
        )
        return bool(renewed)

    async def complete(self, task_ids: list[str], claim_token: str) -> int:
        """Mark claimed tasks completed. Returns the number completed."""
        if len(claim_token) != 26:
            raise ValueError(f"claim_token must be a 26-char ULID, got {claim_token!r}")
        if not task_ids:
            return 0
        pool = await self._get_pool()
        completed = await pool.fetchval(
            "SELECT task_complete_bulk($1, $2)", task_ids, claim_token
        )
        return int(completed)

    async def fail(
        self,
        task_ids: list[str],
        claim_token: str,
        error: str,
        retryable: bool,
        retry_delay_seconds: int = 0,
    ) -> dict[str, TaskStatus]:
        """Fail claimed tasks.

        Retryable tasks with attempt budget left return to pending and become
        claimable again after ``retry_delay_seconds``; the rest become
        dead_letter. Returns the resulting status per task id.
        """
        if len(claim_token) != 26:
            raise ValueError(f"claim_token must be a 26-char ULID, got {claim_token!r}")
        if retry_delay_seconds < 0:
            raise ValueError("fail retry_delay_seconds must be >= 0")
        if not task_ids:
            return {}
        pool = await self._get_pool()
        rows = await pool.fetch(
            "SELECT * FROM task_fail_bulk($1, $2, $3, $4, $5)",
            task_ids,
            claim_token,
            error,
            retryable,
            retry_delay_seconds,
        )
        return {row["task_id"]: TaskStatus(row["result_status"]) for row in rows}

    async def recover_expired(self) -> list[Task]:
        """Recover tasks whose lease expired while running: retryable tasks
        are requeued as pending, exhausted tasks become dead_letter. Returns
        every recovered row so adapters that mirror queue state into domain
        tables can synchronize."""
        pool = await self._get_pool()
        rows = await pool.fetch("SELECT * FROM task_recover_expired()")
        return [Task.from_row(row) for row in rows]

    async def stats(self, task_type: str | None = None) -> list[TaskStats]:
        """Status statistics grouped by (task_type, status)."""
        pool = await self._get_pool()
        rows = await pool.fetch("SELECT * FROM task_stats($1)", task_type)
        return [
            TaskStats(
                task_type=row["task_type"],
                status=TaskStatus(row["status"]),
                count=int(row["count"]),
            )
            for row in rows
        ]

    async def cleanup(self, before: datetime) -> int:
        """Delete terminal (completed/dead_letter) tasks completed before the
        cutoff. Returns the number of deleted rows."""
        pool = await self._get_pool()
        deleted = await pool.fetchval("SELECT task_cleanup($1)", before)
        return int(deleted)

    @staticmethod
    def _validate_enqueue_task_request(task: EnqueueTaskRequest) -> None:
        if task.id is not None and len(task.id) != 26:
            raise ValueError(f"task id must be a 26-char ULID, got {task.id!r}")
        if not task.task_type.strip():
            raise ValueError("task_type must not be empty")
        if not isinstance(task.payload, dict):
            raise ValueError("task payload must be a JSON object")
        if task.weight < 0:
            raise ValueError("task weight must be >= 0")
        if task.max_attempts < 1:
            raise ValueError("max_attempts must be >= 1")
        if task.payload_version < 1:
            raise ValueError("payload_version must be >= 1")
