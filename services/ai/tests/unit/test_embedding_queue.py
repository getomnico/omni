"""Unit tests for the document embedding task adapter."""

from datetime import UTC, datetime

import pytest

from db.embedding_queue import (
    DOCUMENT_EMBEDDING_MAX_ATTEMPTS,
    DOCUMENT_EMBEDDING_PAYLOAD_VERSION,
    DOCUMENT_EMBEDDING_TASK_TYPE,
    EmbeddingQueueItem,
    QueueStatus,
    embedding_task,
)
from db.task_queue import Task

pytestmark = pytest.mark.unit


NOW = datetime.now(UTC)


def _task(payload: dict, **overrides) -> Task:
    row = {
        "id": "01J00000000000000000000000",
        "task_type": DOCUMENT_EMBEDDING_TASK_TYPE,
        "payload": payload,
        "payload_version": DOCUMENT_EMBEDDING_PAYLOAD_VERSION,
        "status": "running",
        "priority": 0,
        "available_at": NOW,
        "weight": 1,
        "concurrency_key": None,
        "deduplication_key": "doc-1",
        "attempt_count": 1,
        "max_attempts": DOCUMENT_EMBEDDING_MAX_ATTEMPTS,
        "last_error": None,
        "claim_token": "01J00000000000000000000001",
        "claimed_by": "worker",
        "lease_expires_at": NOW,
        "created_at": NOW,
        "updated_at": NOW,
        "last_started_at": NOW,
        "completed_at": None,
    }
    row.update(overrides)
    return Task.from_row(row)


def test_embedding_enqueue_request_has_versioned_payload_and_deduplication():
    request = embedding_task("doc-1")

    assert request.task_type == DOCUMENT_EMBEDDING_TASK_TYPE
    assert request.payload_version == DOCUMENT_EMBEDDING_PAYLOAD_VERSION
    assert request.payload == {"document_id": "doc-1"}
    assert request.deduplication_key == "doc-1"
    assert request.max_attempts == DOCUMENT_EMBEDDING_MAX_ATTEMPTS


def test_embedding_payload_is_validated_at_adapter_boundary():
    item = EmbeddingQueueItem.from_task(_task({"document_id": "doc-1"}))
    assert item.document_id == "doc-1"
    assert item.status == QueueStatus.PROCESSING

    with pytest.raises(ValueError, match="requires document_id"):
        EmbeddingQueueItem.from_task(_task({"document_id": 123}))
    with pytest.raises(ValueError, match="payload version"):
        EmbeddingQueueItem.from_task(_task({"document_id": "doc-1"}, payload_version=2))
