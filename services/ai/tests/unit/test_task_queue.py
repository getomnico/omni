"""Unit tests for the Python task queue facade.

The queue lifecycle itself lives in PostgreSQL functions (migration 112) and
is exercised end-to-end through the Rust facade in
``shared/tests/task_queue_test.rs``. These tests cover the Python-specific
parts: argument conversion, JSON payload decoding, row mapping, and error
propagation.
"""

from datetime import UTC, datetime
from unittest.mock import AsyncMock

import pytest

from db.task_queue import (
    ClaimOptions,
    NewTask,
    Task,
    TaskClaim,
    TaskQueueRepository,
    TaskStats,
    TaskStatus,
)

pytestmark = pytest.mark.unit

NOW = datetime(2024, 1, 1, tzinfo=UTC)
TASK_ID = "01J00000000000000000000000"
CLAIM_TOKEN = "01J00000000000000000000001"
ENQUEUE_SQL = "SELECT * FROM task_enqueue_bulk($1, $2, $3, $4, $5, $6, $7, $8, $9)"
CLAIM_SQL = "SELECT * FROM task_claim_bulk($1, $2, $3, $4, $5, $6, $7, $8)"


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


@pytest.mark.asyncio
async def test_enqueue_bulk_builds_arrays_and_maps_tasks():
    pool = AsyncMock()
    pool.fetch = AsyncMock(return_value=[_task_row(weight=4, concurrency_key="k")])
    repo = TaskQueueRepository(pool=pool)

    tasks = await repo.enqueue_bulk(
        [
            NewTask(
                task_type="test",
                payload={"n": 1},
                id=TASK_ID,
                available_at=NOW,
                weight=4,
                concurrency_key="k",
                max_attempts=5,
            )
        ]
    )

    assert len(tasks) == 1
    assert tasks[0].id == TASK_ID
    assert tasks[0].status == TaskStatus.PENDING
    assert tasks[0].payload == {"n": 1}
    assert tasks[0].weight == 4

    sql, *args = pool.fetch.call_args.args
    assert sql == ENQUEUE_SQL
    ids, task_types, payloads, versions, priorities, available_ms, weights, keys, attempts = args
    assert ids == [TASK_ID]
    assert task_types == ["test"]
    assert payloads == ['{"n": 1}']
    assert versions == [1]
    assert priorities == [0]
    assert available_ms == [int(NOW.timestamp() * 1000)]
    assert weights == [4]
    assert keys == ["k"]
    assert attempts == [5]


@pytest.mark.asyncio
async def test_enqueue_generates_ulid_when_id_missing():
    pool = AsyncMock()
    pool.fetch = AsyncMock(return_value=[_task_row()])
    repo = TaskQueueRepository(pool=pool)

    await repo.enqueue_bulk([NewTask(task_type="test", payload={})])

    ids = pool.fetch.call_args.args[1]
    assert len(ids) == 1
    assert len(ids[0]) == 26


@pytest.mark.asyncio
async def test_enqueue_returns_existing_task_on_idempotent_retry():
    pool = AsyncMock()
    pool.fetch = AsyncMock(return_value=[])  # duplicate id: nothing inserted
    pool.fetchrow = AsyncMock(return_value=_task_row(status="completed"))
    repo = TaskQueueRepository(pool=pool)

    task = await repo.enqueue(NewTask(task_type="test", payload={}, id=TASK_ID))

    assert task.id == TASK_ID
    assert task.status == TaskStatus.COMPLETED
    assert pool.fetchrow.call_args.args[1] == TASK_ID


@pytest.mark.asyncio
async def test_enqueue_validation_raises():
    pool = AsyncMock()
    repo = TaskQueueRepository(pool=pool)

    with pytest.raises(ValueError, match="26-char"):
        await repo.enqueue_bulk([NewTask(task_type="test", payload={}, id="short")])
    with pytest.raises(ValueError, match="task_type"):
        await repo.enqueue_bulk([NewTask(task_type="  ", payload={})])
    with pytest.raises(ValueError, match="payload"):
        await repo.enqueue_bulk([NewTask(task_type="test", payload="nope")])  # type: ignore
    with pytest.raises(ValueError, match="weight"):
        await repo.enqueue_bulk([NewTask(task_type="test", payload={}, weight=-1)])
    with pytest.raises(ValueError, match="max_attempts"):
        await repo.enqueue_bulk([NewTask(task_type="test", payload={}, max_attempts=0)])

    assert pool.fetch.await_count == 0


@pytest.mark.asyncio
async def test_claim_returns_token_and_maps_tasks():
    pool = AsyncMock()
    pool.fetch = AsyncMock(
        return_value=[
            _task_row(
                status="running",
                claim_token="01J00000000000000000000000",
                claimed_by="worker-1",
                lease_expires_at=NOW,
                attempt_count=1,
            )
        ]
    )
    repo = TaskQueueRepository(pool=pool)

    claim = await repo.claim(
        "test", "worker-1", ClaimOptions(limit=1, lease_seconds=120)
    )

    assert isinstance(claim, TaskClaim)
    assert len(claim.claim_token) == 26
    assert len(claim.tasks) == 1
    assert claim.tasks[0].status == TaskStatus.RUNNING
    assert claim.tasks[0].claim_token == TASK_ID  # value from the mocked row
    assert claim.tasks[0].claimed_by == "worker-1"

    sql, *args = pool.fetch.call_args.args
    assert sql == CLAIM_SQL
    task_type, candidate_ids, limit, max_weight, max_concurrency, lease, token, claimed_by = args
    assert task_type == "test"
    assert candidate_ids is None
    assert limit == 1
    assert max_weight is None
    assert max_concurrency is None
    assert lease == 120
    assert len(token) == 26
    assert claimed_by == "worker-1"


@pytest.mark.asyncio
async def test_claim_uses_provided_connection():
    pool = AsyncMock()
    connection = AsyncMock()
    connection.fetch = AsyncMock(return_value=[])
    repo = TaskQueueRepository(pool=pool)

    claim = await repo.claim("test", "w", ClaimOptions(), connection=connection)

    assert claim.tasks == []
    connection.fetch.assert_awaited_once()
    assert pool.fetch.await_count == 0


@pytest.mark.asyncio
async def test_claim_validation_raises():
    repo = TaskQueueRepository(pool=AsyncMock())
    with pytest.raises(ValueError, match="task_type"):
        await repo.claim("", "w", ClaimOptions())
    with pytest.raises(ValueError, match="claimed_by"):
        await repo.claim("test", "", ClaimOptions())
    with pytest.raises(ValueError, match="limit"):
        await repo.claim("test", "w", ClaimOptions(limit=0))
    with pytest.raises(ValueError, match="max_weight"):
        await repo.claim("test", "w", ClaimOptions(max_weight=-1))
    with pytest.raises(ValueError, match="max_concurrency"):
        await repo.claim("test", "w", ClaimOptions(max_concurrency=0))
    with pytest.raises(ValueError, match="lease_seconds"):
        await repo.claim("test", "w", ClaimOptions(lease_seconds=0))
    with pytest.raises(ValueError, match="26-char"):
        await repo.claim("test", "w", ClaimOptions(candidate_ids=["short"]))


@pytest.mark.asyncio
async def test_heartbeat_returns_bool():
    pool = AsyncMock()
    pool.fetchval = AsyncMock(return_value=True)
    repo = TaskQueueRepository(pool=pool)

    assert await repo.heartbeat(TASK_ID, CLAIM_TOKEN, 60) is True
    assert pool.fetchval.call_args.args[1:] == (TASK_ID, CLAIM_TOKEN, 60)

    pool.fetchval = AsyncMock(return_value=False)
    assert await repo.heartbeat(TASK_ID, CLAIM_TOKEN, 60) is False


@pytest.mark.asyncio
async def test_heartbeat_and_fail_validate_durations():
    repo = TaskQueueRepository(pool=AsyncMock())
    with pytest.raises(ValueError, match="lease"):
        await repo.heartbeat(TASK_ID, CLAIM_TOKEN, 0)
    with pytest.raises(ValueError, match="retry_delay"):
        await repo.fail([TASK_ID], CLAIM_TOKEN, "boom", True, -1)
    with pytest.raises(ValueError, match="claim_token"):
        await repo.heartbeat(TASK_ID, "short", 60)
    with pytest.raises(ValueError, match="task id"):
        await repo.heartbeat("short", CLAIM_TOKEN, 60)
    with pytest.raises(ValueError, match="claim_token"):
        await repo.complete([TASK_ID], "short")
    with pytest.raises(ValueError, match="claim_token"):
        await repo.fail([TASK_ID], "short", "boom", True)


@pytest.mark.asyncio
async def test_complete_returns_count_and_short_circuits_empty():
    pool = AsyncMock()
    pool.fetchval = AsyncMock(return_value=2)
    repo = TaskQueueRepository(pool=pool)

    assert await repo.complete([TASK_ID, "01J00000000000000000000001"], CLAIM_TOKEN) == 2
    assert pool.fetchval.call_args.args[1:] == (
        ["01J00000000000000000000000", "01J00000000000000000000001"],
        CLAIM_TOKEN,
    )

    assert await repo.complete([], CLAIM_TOKEN) == 0
    assert pool.fetchval.await_count == 1


@pytest.mark.asyncio
async def test_fail_maps_statuses_per_task():
    pool = AsyncMock()
    pool.fetch = AsyncMock(
        return_value=[
            {"task_id": TASK_ID, "result_status": "pending"},
            {"task_id": "01J00000000000000000000002", "result_status": "dead_letter"},
        ]
    )
    repo = TaskQueueRepository(pool=pool)

    result = await repo.fail(
        [TASK_ID, "01J00000000000000000000002"], CLAIM_TOKEN, "boom", True, 30
    )

    assert result == {
        TASK_ID: TaskStatus.PENDING,
        "01J00000000000000000000002": TaskStatus.DEAD_LETTER,
    }
    sql, *args = pool.fetch.call_args.args
    assert sql == "SELECT * FROM task_fail_bulk($1, $2, $3, $4, $5)"
    assert args[2] == "boom"
    assert args[3] is True
    assert args[4] == 30

    assert await repo.fail([], CLAIM_TOKEN, "boom", True) == {}
    assert pool.fetch.await_count == 1


@pytest.mark.asyncio
async def test_recover_expired_maps_rows():
    pool = AsyncMock()
    pool.fetch = AsyncMock(return_value=[_task_row(status="pending", last_error="lease expired")])
    repo = TaskQueueRepository(pool=pool)

    recovered = await repo.recover_expired()

    assert len(recovered) == 1
    assert recovered[0].status == TaskStatus.PENDING
    assert recovered[0].last_error == "lease expired"
    assert pool.fetch.call_args.args[0] == "SELECT * FROM task_recover_expired()"


@pytest.mark.asyncio
async def test_stats_maps_rows():
    pool = AsyncMock()
    pool.fetch = AsyncMock(
        return_value=[
            {"task_type": "test", "status": "pending", "count": 3},
            {"task_type": "test", "status": "completed", "count": 1},
        ]
    )
    repo = TaskQueueRepository(pool=pool)

    stats = await repo.stats("test")

    assert stats == [
        TaskStats(task_type="test", status=TaskStatus.PENDING, count=3),
        TaskStats(task_type="test", status=TaskStatus.COMPLETED, count=1),
    ]
    assert pool.fetch.call_args.args[1] == "test"


@pytest.mark.asyncio
async def test_cleanup_returns_count():
    pool = AsyncMock()
    pool.fetchval = AsyncMock(return_value=5)
    repo = TaskQueueRepository(pool=pool)

    assert await repo.cleanup(NOW) == 5
    assert pool.fetchval.call_args.args[1] == NOW


@pytest.mark.asyncio
async def test_from_row_parses_string_payload_and_status():
    task = Task.from_row(_task_row(payload='{"n": 1}', status="running"))

    assert task.payload == {"n": 1}
    assert task.status == TaskStatus.RUNNING


@pytest.mark.asyncio
async def test_database_error_propagates():
    pool = AsyncMock()
    pool.fetch = AsyncMock(side_effect=OSError("connection lost"))
    repo = TaskQueueRepository(pool=pool)

    with pytest.raises(OSError, match="connection lost"):
        await repo.enqueue_bulk([NewTask(task_type="test", payload={})])
