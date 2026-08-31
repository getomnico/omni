"""Pure unit tests for task queue validation and row conversion."""

from datetime import UTC, datetime

import pytest

from db.task_queue import (
    ClaimOptions,
    EnqueueTaskRequest,
    Task,
    TaskQueueRepository,
    TaskStatus,
    _to_epoch_ms,
)

pytestmark = pytest.mark.unit

NOW = datetime(2024, 1, 1, tzinfo=UTC)
TASK_ID = "01J00000000000000000000000"
CLAIM_TOKEN = "01J00000000000000000000001"


def _task_row(**overrides) -> dict:
    row = {
        "id": TASK_ID,
        "task_type": "test",
        "payload": {"n": 1},
        "payload_version": 1,
        "status": "pending",
        "priority": 0,
        "available_at": NOW,
        "weight": 1,
        "concurrency_key": None,
        "attempt_count": 0,
        "max_attempts": 3,
        "last_error": None,
        "claim_token": None,
        "claimed_by": None,
        "lease_expires_at": None,
        "created_at": NOW,
        "updated_at": NOW,
        "last_started_at": None,
        "completed_at": None,
    }
    row.update(overrides)
    return row


def test_to_epoch_ms_handles_aware_and_naive_datetimes():
    expected = int(NOW.timestamp() * 1000)

    assert _to_epoch_ms(NOW) == expected
    assert _to_epoch_ms(NOW.replace(tzinfo=None)) == expected


def test_from_row_parses_string_payload_and_status():
    task = Task.from_row(_task_row(payload='{"n": 1}', status="running"))

    assert task.payload == {"n": 1}
    assert task.status == TaskStatus.RUNNING


@pytest.mark.asyncio
async def test_enqueue_validation_raises_before_database_access():
    repo = TaskQueueRepository()

    with pytest.raises(ValueError, match="26-char"):
        await repo.enqueue_bulk(
            [EnqueueTaskRequest(task_type="test", payload={}, id="short")]
        )
    with pytest.raises(ValueError, match="task_type"):
        await repo.enqueue_bulk([EnqueueTaskRequest(task_type="  ", payload={})])
    with pytest.raises(ValueError, match="payload"):
        await repo.enqueue_bulk(
            [EnqueueTaskRequest(task_type="test", payload="nope")]  # type: ignore
        )
    with pytest.raises(ValueError, match="weight"):
        await repo.enqueue_bulk(
            [EnqueueTaskRequest(task_type="test", payload={}, weight=-1)]
        )
    with pytest.raises(ValueError, match="max_attempts"):
        await repo.enqueue_bulk(
            [EnqueueTaskRequest(task_type="test", payload={}, max_attempts=0)]
        )
    with pytest.raises(ValueError, match="payload_version"):
        await repo.enqueue_bulk(
            [EnqueueTaskRequest(task_type="test", payload={}, payload_version=0)]
        )


@pytest.mark.asyncio
async def test_claim_validation_raises_before_database_access():
    repo = TaskQueueRepository()

    with pytest.raises(ValueError, match="task_type"):
        await repo.claim("", "worker", ClaimOptions())
    with pytest.raises(ValueError, match="claimed_by"):
        await repo.claim("test", "", ClaimOptions())
    with pytest.raises(ValueError, match="limit"):
        await repo.claim("test", "worker", ClaimOptions(limit=0))
    with pytest.raises(ValueError, match="max_weight"):
        await repo.claim("test", "worker", ClaimOptions(max_weight=-1))
    with pytest.raises(ValueError, match="max_concurrency"):
        await repo.claim("test", "worker", ClaimOptions(max_concurrency=0))
    with pytest.raises(ValueError, match="lease_seconds"):
        await repo.claim("test", "worker", ClaimOptions(lease_seconds=0))
    with pytest.raises(ValueError, match="26-char"):
        await repo.claim("test", "worker", ClaimOptions(candidate_ids=["short"]))


@pytest.mark.asyncio
async def test_terminal_operation_validation_raises_before_database_access():
    repo = TaskQueueRepository()

    with pytest.raises(ValueError, match="task id"):
        await repo.heartbeat("short", CLAIM_TOKEN, 60)
    with pytest.raises(ValueError, match="claim_token"):
        await repo.heartbeat(TASK_ID, "short", 60)
    with pytest.raises(ValueError, match="lease"):
        await repo.heartbeat(TASK_ID, CLAIM_TOKEN, 0)
    with pytest.raises(ValueError, match="claim_token"):
        await repo.complete([TASK_ID], "short")
    with pytest.raises(ValueError, match="claim_token"):
        await repo.fail([TASK_ID], "short", "boom", True)
    with pytest.raises(ValueError, match="retry_delay"):
        await repo.fail([TASK_ID], CLAIM_TOKEN, "boom", True, -1)


@pytest.mark.asyncio
async def test_empty_terminal_batches_short_circuit_without_database():
    repo = TaskQueueRepository()

    assert await repo.complete([], CLAIM_TOKEN) == 0
    assert await repo.fail([], CLAIM_TOKEN, "boom", True) == {}
