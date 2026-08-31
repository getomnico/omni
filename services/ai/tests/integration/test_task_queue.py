"""PostgreSQL-backed integration tests for the Python task queue facade."""

from datetime import UTC, datetime, timedelta

import pytest
from ulid import ULID

from db.task_queue import (
    ClaimOptions,
    EnqueueTaskRequest,
    TaskQueueRepository,
    TaskStatus,
)

pytestmark = pytest.mark.integration


def _unique_task_type(prefix: str) -> str:
    return f"{prefix}_{ULID()}"


@pytest.mark.asyncio
async def test_enqueue_round_trip_generated_id_and_idempotent_retry(db_pool):
    repo = TaskQueueRepository(pool=db_pool)
    explicit_id = str(ULID())
    explicit_type = _unique_task_type("enqueue_explicit")
    generated_type = _unique_task_type("enqueue_generated")
    available_at = datetime.now(UTC) + timedelta(seconds=5)

    created = await repo.enqueue_bulk(
        [
            EnqueueTaskRequest(
                id=explicit_id,
                task_type=explicit_type,
                payload={"nested": {"enabled": True}, "items": [1, "two"]},
                payload_version=2,
                priority=4,
                available_at=available_at,
                weight=7,
                concurrency_key="partition-1",
                max_attempts=5,
            ),
            EnqueueTaskRequest(task_type=generated_type, payload={"generated": True}),
        ]
    )

    assert len(created) == 2
    by_type = {task.task_type: task for task in created}
    explicit = by_type[explicit_type]
    generated = by_type[generated_type]

    assert explicit.id == explicit_id
    assert explicit.payload == {"nested": {"enabled": True}, "items": [1, "two"]}
    assert explicit.payload_version == 2
    assert explicit.priority == 4
    assert explicit.weight == 7
    assert explicit.concurrency_key == "partition-1"
    assert explicit.max_attempts == 5
    assert abs((explicit.available_at - available_at).total_seconds()) < 0.01
    assert len(generated.id) == 26
    assert generated.payload == {"generated": True}

    retried = await repo.enqueue(
        EnqueueTaskRequest(
            id=explicit_id, task_type=explicit_type, payload={"replacement": True}
        )
    )
    assert retried.id == explicit_id
    assert retried.payload == explicit.payload
    assert retried.payload_version == explicit.payload_version


@pytest.mark.asyncio
async def test_claim_uses_provided_transaction(db_pool):
    repo = TaskQueueRepository(pool=db_pool)
    task_type = _unique_task_type("claim_transaction")
    task = await repo.enqueue(
        EnqueueTaskRequest(task_type=task_type, payload={"n": 1})
    )

    async with db_pool.acquire() as connection:
        transaction = connection.transaction()
        await transaction.start()
        try:
            claim = await repo.claim(
                task_type,
                "worker-transaction",
                ClaimOptions(limit=1, lease_seconds=60),
                connection=connection,
            )
            assert len(claim.tasks) == 1
            assert claim.tasks[0].id == task.id
            assert claim.tasks[0].status == TaskStatus.RUNNING
            assert claim.tasks[0].attempt_count == 1
        finally:
            await transaction.rollback()

    rolled_back = await repo.get(task.id)
    assert rolled_back is not None
    assert rolled_back.status == TaskStatus.PENDING
    assert rolled_back.attempt_count == 0


@pytest.mark.asyncio
async def test_claim_heartbeat_and_complete_lifecycle(db_pool):
    repo = TaskQueueRepository(pool=db_pool)
    task_type = _unique_task_type("complete_lifecycle")
    task = await repo.enqueue(
        EnqueueTaskRequest(task_type=task_type, payload={"n": 1})
    )

    claim = await repo.claim(
        task_type,
        "worker-complete",
        ClaimOptions(limit=1, lease_seconds=60),
    )
    assert len(claim.claim_token) == 26
    assert [claimed.id for claimed in claim.tasks] == [task.id]
    assert claim.tasks[0].status == TaskStatus.RUNNING

    assert await repo.heartbeat(task.id, claim.claim_token, 120) is True
    assert await repo.complete([task.id], claim.claim_token) == 1
    assert await repo.complete([task.id], claim.claim_token) == 0

    completed = await repo.get(task.id)
    assert completed is not None
    assert completed.status == TaskStatus.COMPLETED
    assert completed.completed_at is not None
    assert completed.claim_token is None
    assert completed.claimed_by is None
    assert completed.lease_expires_at is None


@pytest.mark.asyncio
async def test_retry_and_dead_letter_lifecycle(db_pool):
    repo = TaskQueueRepository(pool=db_pool)
    task_type = _unique_task_type("fail_lifecycle")
    task = await repo.enqueue(
        EnqueueTaskRequest(task_type=task_type, payload={"n": 1}, max_attempts=2)
    )

    first_claim = await repo.claim(task_type, "worker-fail", ClaimOptions())
    first_result = await repo.fail(
        [task.id], first_claim.claim_token, "temporary", retryable=True
    )
    assert first_result == {task.id: TaskStatus.PENDING}

    pending = await repo.get(task.id)
    assert pending is not None
    assert pending.status == TaskStatus.PENDING
    assert pending.attempt_count == 1
    assert pending.last_error == "temporary"

    second_claim = await repo.claim(task_type, "worker-fail", ClaimOptions())
    second_result = await repo.fail(
        [task.id], second_claim.claim_token, "still failing", retryable=True
    )
    assert second_result == {task.id: TaskStatus.DEAD_LETTER}

    dead_letter = await repo.get(task.id)
    assert dead_letter is not None
    assert dead_letter.status == TaskStatus.DEAD_LETTER
    assert dead_letter.attempt_count == 2
    assert dead_letter.completed_at is not None
    assert dead_letter.last_error == "still failing"


@pytest.mark.asyncio
async def test_recover_expired_maps_real_rows(db_pool):
    repo = TaskQueueRepository(pool=db_pool)
    task_type = _unique_task_type("recover_expired")
    task = await repo.enqueue(
        EnqueueTaskRequest(task_type=task_type, payload={"n": 1}, max_attempts=2)
    )
    claim = await repo.claim(task_type, "worker-recover", ClaimOptions())
    assert len(claim.tasks) == 1

    await db_pool.execute(
        "UPDATE tasks SET lease_expires_at = clock_timestamp() - INTERVAL '1 second' WHERE id = $1",
        task.id,
    )
    recovered = await repo.recover_expired()
    own_task = next(item for item in recovered if item.id == task.id)

    assert own_task.status == TaskStatus.PENDING
    assert own_task.attempt_count == 1
    assert own_task.claim_token is None
    assert own_task.claimed_by is None
    assert own_task.lease_expires_at is None
    assert own_task.last_error == "task lease expired; requeued"


@pytest.mark.asyncio
async def test_stats_and_cleanup(db_pool):
    repo = TaskQueueRepository(pool=db_pool)
    task_type = _unique_task_type("stats_cleanup")
    first = await repo.enqueue(
        EnqueueTaskRequest(task_type=task_type, payload={"n": 1})
    )
    second = await repo.enqueue(
        EnqueueTaskRequest(task_type=task_type, payload={"n": 2})
    )

    claim = await repo.claim(task_type, "worker-stats", ClaimOptions(limit=1))
    completed_id = claim.tasks[0].id
    pending_id = second.id if completed_id == first.id else first.id
    assert await repo.complete([completed_id], claim.claim_token) == 1

    stats = await repo.stats(task_type)
    counts = {row.status: row.count for row in stats}
    assert counts == {TaskStatus.COMPLETED: 1, TaskStatus.PENDING: 1}

    await db_pool.execute(
        "UPDATE tasks SET completed_at = clock_timestamp() - INTERVAL '7 days' WHERE id = $1",
        completed_id,
    )
    deleted = await repo.cleanup(datetime.now(UTC) - timedelta(days=1))
    assert deleted >= 1
    assert await repo.get(completed_id) is None

    pending = await repo.get(pending_id)
    assert pending is not None
    assert pending.status == TaskStatus.PENDING
