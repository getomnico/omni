"""Integration tests for the EmbeddingBatchProcessor.

Tests the embedding processor with real database and a mocked embedding provider.
"""

from unittest.mock import AsyncMock, MagicMock

import pytest
import ulid

from embeddings.batch_processor import EmbeddingBatchProcessor
from state import AppState
from tests.helpers import (
    create_test_document as create_document_record,
)
from tests.helpers import (
    create_test_document_with_content as create_test_document,
)
from tests.helpers import (
    create_test_source,
    enqueue_document,
)
from tests.helpers import (
    create_test_user as _create_test_user_full,
)


async def create_test_user(db_pool) -> str:
    """Wrapper that returns just user_id (batch processor tests don't need email)."""
    user_id, _ = await _create_test_user_full(db_pool)
    return user_id


def _build_app_state(
    db_pool, embedding_provider, provider_type: str = "jina"
) -> AppState:
    """Build a minimal AppState wired to a DB-backed content_storage and a mock provider."""
    content_storage = AsyncMock()

    async def get_text_from_db(content_id):
        async with db_pool.acquire() as conn:
            row = await conn.fetchrow(
                "SELECT content FROM content_blobs WHERE id = $1", content_id
            )
            return row["content"].decode() if row else None

    content_storage.get_text = get_text_from_db

    state = AppState()
    state.embedding_provider = embedding_provider
    state.embedding_provider_type = provider_type
    state.content_storage = content_storage
    return state


# =============================================================================
# Processor Fixtures
# =============================================================================


@pytest.fixture
async def online_processor(
    db_pool,
    documents_repo,
    queue_repo,
    embeddings_repo,
    mock_embedding_provider,
):
    """Processor with real DB repos, mocked embedding provider."""
    return EmbeddingBatchProcessor(
        documents_repo=documents_repo,
        queue_repo=queue_repo,
        embeddings_repo=embeddings_repo,
        app_state=_build_app_state(db_pool, mock_embedding_provider),
    )


# =============================================================================
# Online Processing Tests (Real DB)
# =============================================================================


@pytest.mark.integration
async def test_online_processes_document_end_to_end(
    db_pool, online_processor, queue_repo, embeddings_repo
):
    """Full flow: queue item -> fetch -> embed -> store in DB -> mark complete."""
    user_id = await create_test_user(db_pool)
    source_id = await create_test_source(db_pool, user_id)
    doc_id = await create_test_document(
        db_pool, source_id, "Test content for embedding."
    )
    queue_id = await enqueue_document(db_pool, doc_id)

    await online_processor._process_online_batch()

    embeddings = await embeddings_repo.get_for_document(doc_id)
    assert len(embeddings) >= 1
    assert len(embeddings[0].embedding) == 1024

    queue_item = await queue_repo.get_by_id(queue_id)
    assert queue_item.status == "completed"


@pytest.mark.integration
async def test_online_clones_embeddings_for_duplicate_content(
    db_pool,
    online_processor,
    queue_repo,
    embeddings_repo,
    mock_embedding_provider,
):
    """Same-content documents clone existing embeddings instead of calling provider."""
    user_id = await create_test_user(db_pool)
    source_id = await create_test_source(db_pool, user_id)
    content_id = str(ulid.ULID())
    donor_doc_id = await create_document_record(
        db_pool,
        source_id,
        "Donor Document",
        "Duplicate content for cloning.",
        content_id=content_id,
        external_id="donor-duplicate-content",
    )
    target_doc_id = str(ulid.ULID())
    async with db_pool.acquire() as conn:
        await conn.execute(
            """
            INSERT INTO documents (id, source_id, external_id, title, content_id, content)
            VALUES ($1, $2, $3, $4, $5, $6)
            """,
            target_doc_id,
            source_id,
            "target-duplicate-content",
            "Target Document",
            content_id,
            "Duplicate content for cloning.",
        )

    await embeddings_repo.bulk_insert(
        [
            {
                "id": str(ulid.ULID()),
                "document_id": donor_doc_id,
                "chunk_index": 0,
                "chunk_start_offset": 0,
                "chunk_end_offset": 30,
                "embedding": [0.1, 0.2, 0.3],
                "model_name": "test-embedding-model",
                "dimensions": 3,
            }
        ]
    )
    queue_id = await enqueue_document(db_pool, target_doc_id)

    await online_processor._process_online_batch()

    queue_item = await queue_repo.get_by_id(queue_id)
    assert queue_item.status == "completed"

    target_embeddings = await embeddings_repo.get_for_document(target_doc_id)
    assert len(target_embeddings) == 1
    assert target_embeddings[0].chunk_index == 0
    assert target_embeddings[0].model_name == "test-embedding-model"
    mock_embedding_provider.generate_embeddings.assert_not_called()


@pytest.mark.integration
async def test_online_does_not_clone_from_donor_with_unresolved_embedding_work(
    db_pool,
    online_processor,
    queue_repo,
    embeddings_repo,
    mock_embedding_provider,
):
    """A donor with active embedding queue work may have stale embeddings."""
    user_id = await create_test_user(db_pool)
    source_id = await create_test_source(db_pool, user_id)
    content_id = str(ulid.ULID())
    donor_doc_id = await create_document_record(
        db_pool,
        source_id,
        "Donor Document",
        "Duplicate content with stale donor risk.",
        content_id=content_id,
        external_id="donor-unresolved-content",
    )
    target_doc_id = str(ulid.ULID())
    async with db_pool.acquire() as conn:
        await conn.execute(
            """
            INSERT INTO documents (id, source_id, external_id, title, content_id, content)
            VALUES ($1, $2, $3, $4, $5, $6)
            """,
            target_doc_id,
            source_id,
            "target-unresolved-content",
            "Target Document",
            content_id,
            "Duplicate content with stale donor risk.",
        )
        await conn.execute(
            """
            INSERT INTO embedding_queue (id, document_id, status)
            VALUES ($1, $2, 'processing')
            """,
            str(ulid.ULID()),
            donor_doc_id,
        )

    await embeddings_repo.bulk_insert(
        [
            {
                "id": str(ulid.ULID()),
                "document_id": donor_doc_id,
                "chunk_index": 0,
                "chunk_start_offset": 0,
                "chunk_end_offset": 42,
                "embedding": [0.4, 0.5, 0.6],
                "model_name": "test-embedding-model",
                "dimensions": 3,
            }
        ]
    )
    queue_id = await enqueue_document(db_pool, target_doc_id)

    await online_processor._process_online_batch()

    queue_item = await queue_repo.get_by_id(queue_id)
    assert queue_item.status == "completed"
    target_embeddings = await embeddings_repo.get_for_document(target_doc_id)
    assert len(target_embeddings) == 1
    assert len(target_embeddings[0].embedding) == 1024
    mock_embedding_provider.generate_embeddings.assert_called_once()


@pytest.mark.integration
async def test_online_handles_empty_content(
    db_pool, online_processor, queue_repo, embeddings_repo
):
    """Empty document content marks queue item as failed."""
    user_id = await create_test_user(db_pool)
    source_id = await create_test_source(db_pool, user_id)
    doc_id = await create_test_document(db_pool, source_id, "")
    queue_id = await enqueue_document(db_pool, doc_id)

    await online_processor._process_online_batch()

    queue_item = await queue_repo.get_by_id(queue_id)
    assert queue_item.status == "failed"
    assert queue_item.error_message is not None

    embeddings = await embeddings_repo.get_for_document(doc_id)
    assert len(embeddings) == 0


# =============================================================================
# Large Document Handling Tests
# =============================================================================


@pytest.fixture
async def online_processor_with_sliding_window(
    db_pool,
    documents_repo,
    queue_repo,
    embeddings_repo,
):
    """Processor with a mock embedding provider that tracks calls for large doc testing."""
    provider = AsyncMock()
    provider.get_model_name = MagicMock(return_value="test-embedding-model")

    async def generate_with_spans(text, **kwargs):
        mock_chunk = MagicMock()
        mock_chunk.span = (0, len(text))
        mock_chunk.embedding = [0.1] * 1024
        return [mock_chunk]

    provider.generate_embeddings.side_effect = generate_with_spans

    return EmbeddingBatchProcessor(
        documents_repo=documents_repo,
        queue_repo=queue_repo,
        embeddings_repo=embeddings_repo,
        app_state=_build_app_state(db_pool, provider),
    )


@pytest.mark.integration
async def test_online_processes_large_document_with_sliding_window(
    db_pool,
    online_processor_with_sliding_window,
    queue_repo,
    embeddings_repo,
    monkeypatch,
):
    """Large documents are split via sliding window and each window is embedded."""
    import embeddings.batch_processor as bp

    monkeypatch.setattr(bp, "EMBEDDING_MAX_MODEL_LEN", 33)

    user_id = await create_test_user(db_pool)
    source_id = await create_test_source(db_pool, user_id)

    # 500 chars -> window_size=100, overlap=25, stride=75
    # Windows at offsets: 0, 75, 150, 225, 300, 375, 450
    large_content = "This is a test sentence. " * 20  # 500 chars
    doc_id = await create_test_document(db_pool, source_id, large_content)
    queue_id = await enqueue_document(db_pool, doc_id)

    await online_processor_with_sliding_window._process_online_batch()

    queue_item = await queue_repo.get_by_id(queue_id)
    assert queue_item.status == "completed"

    embeddings = await embeddings_repo.get_for_document(doc_id)
    assert len(embeddings) == 7

    expected_spans = [
        (0, 99),
        (75, 174),
        (150, 249),
        (225, 324),
        (300, 399),
        (375, 474),
        (450, 500),
    ]
    actual_spans = [(e.chunk_start_offset, e.chunk_end_offset) for e in embeddings]
    assert actual_spans == expected_spans

    for emb in embeddings:
        assert len(emb.embedding) == 1024

    provider = online_processor_with_sliding_window.embedding_provider
    assert provider.generate_embeddings.call_count == 7


# =============================================================================
# Retry Behavior Tests
# =============================================================================


@pytest.mark.integration
async def test_failed_items_are_retried(
    db_pool,
    online_processor,
    queue_repo,
    embeddings_repo,
    mock_embedding_provider,
    monkeypatch,
):
    """Failed items are immediately eligible for retry on next poll."""
    import embeddings.batch_processor as bp

    monkeypatch.setattr(bp, "ONLINE_POLL_INTERVAL", 0)

    user_id = await create_test_user(db_pool)
    source_id = await create_test_source(db_pool, user_id)
    doc_id = await create_test_document(
        db_pool, source_id, "Content that will fail then succeed."
    )
    queue_id = await enqueue_document(db_pool, doc_id)

    mock_chunk = MagicMock()
    mock_chunk.span = (0, 100)
    mock_chunk.embedding = [0.1] * 1024

    mock_embedding_provider.generate_embeddings.side_effect = [
        RuntimeError("Transient API error"),
        [mock_chunk],
    ]

    # 1) First processing attempt — should fail
    await online_processor._process_online_batch()

    item = await queue_repo.get_by_id(queue_id)
    assert item.status == "failed"
    assert item.retry_count == 1

    # 2) Immediate retry — should succeed now
    await online_processor._process_online_batch()

    item = await queue_repo.get_by_id(queue_id)
    assert item.status == "completed"

    embeddings = await embeddings_repo.get_for_document(doc_id)
    assert len(embeddings) >= 1


@pytest.mark.integration
async def test_max_retries_exhausted_items_are_not_retried(
    db_pool,
    online_processor,
    queue_repo,
    monkeypatch,
):
    """Items that have exhausted all retries (retry_count >= 5) are never picked up."""
    import embeddings.batch_processor as bp

    monkeypatch.setattr(bp, "ONLINE_POLL_INTERVAL", 0)

    user_id = await create_test_user(db_pool)
    source_id = await create_test_source(db_pool, user_id)
    doc_id = await create_test_document(
        db_pool, source_id, "Content with exhausted retries."
    )

    queue_id = str(ulid.ULID())
    async with db_pool.acquire() as conn:
        await conn.execute(
            """INSERT INTO embedding_queue (id, document_id, status, retry_count)
               VALUES ($1, $2, 'failed', 5)""",
            queue_id,
            doc_id,
        )

    await online_processor._process_online_batch()

    item = await queue_repo.get_by_id(queue_id)
    assert item.status == "failed"
    assert item.retry_count == 5
