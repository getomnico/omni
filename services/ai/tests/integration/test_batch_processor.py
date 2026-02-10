"""Integration tests for the EmbeddingBatchProcessor.

Tests the batch embedding processor with real database and mocked external services
(S3, Bedrock API, embedding providers).
"""

import pytest
import ulid
from unittest.mock import AsyncMock, MagicMock

from embeddings.batch_processor import EmbeddingBatchProcessor


# =============================================================================
# Test Data Setup Helpers
# =============================================================================


async def create_test_user(db_pool) -> str:
    """Create a test user and return its ID."""
    user_id = str(ulid.ULID())
    async with db_pool.acquire() as conn:
        await conn.execute(
            """INSERT INTO users (id, email, password_hash)
               VALUES ($1, $2, $3)""",
            user_id,
            f"test-{user_id}@example.com",
            "hashed_password_placeholder",
        )
    return user_id


async def create_test_source(db_pool, user_id: str) -> str:
    """Create a test source and return its ID."""
    source_id = str(ulid.ULID())
    async with db_pool.acquire() as conn:
        await conn.execute(
            """INSERT INTO sources (id, name, source_type, created_by)
               VALUES ($1, $2, $3, $4)""",
            source_id,
            "test-source",
            "local_files",
            user_id,
        )
    return source_id


async def create_test_document(db_pool, source_id: str, content: str) -> str:
    """Create a test document with content blob and return its ID."""
    doc_id = str(ulid.ULID())
    content_id = str(ulid.ULID())
    content_bytes = content.encode("utf-8")
    async with db_pool.acquire() as conn:
        # Create content blob (required by batch processor)
        await conn.execute(
            """INSERT INTO content_blobs (id, content, size_bytes, storage_backend)
               VALUES ($1, $2, $3, 'postgres')""",
            content_id,
            content_bytes,
            len(content_bytes),
        )
        # Create document linking to content blob
        await conn.execute(
            """INSERT INTO documents (id, source_id, external_id, title, content_id, content, embedding_status)
               VALUES ($1, $2, $3, $4, $5, $6, 'pending')""",
            doc_id,
            source_id,
            f"test-{doc_id}",
            "Test Document",
            content_id,
            content,
        )
    return doc_id


async def enqueue_document(db_pool, document_id: str) -> str:
    """Add document to embedding queue and return queue item ID."""
    item_id = str(ulid.ULID())
    async with db_pool.acquire() as conn:
        await conn.execute(
            """INSERT INTO embedding_queue (id, document_id, status)
               VALUES ($1, $2, 'pending')""",
            item_id,
            document_id,
        )
    return item_id


# =============================================================================
# Processor Fixtures
# =============================================================================


@pytest.fixture
async def online_processor(
    db_pool,
    documents_repo,
    queue_repo,
    embeddings_repo,
    batch_jobs_repo,
    mock_embedding_provider,
):
    """Processor with real DB repos, mocked embedding provider."""
    # Mock content storage to fetch from DB
    content_storage = AsyncMock()

    async def get_text_from_db(content_id):
        async with db_pool.acquire() as conn:
            row = await conn.fetchrow(
                "SELECT content FROM content_blobs WHERE id = $1", content_id
            )
            return row["content"].decode() if row else None

    content_storage.get_text = get_text_from_db

    return EmbeddingBatchProcessor(
        documents_repo=documents_repo,
        queue_repo=queue_repo,
        embeddings_repo=embeddings_repo,
        batch_jobs_repo=batch_jobs_repo,
        content_storage=content_storage,
        embedding_provider=mock_embedding_provider,
        provider_type="jina",
    )


@pytest.fixture
async def bedrock_processor(
    db_pool,
    documents_repo,
    queue_repo,
    embeddings_repo,
    batch_jobs_repo,
    mock_embedding_provider,
):
    """Bedrock processor with real DB, mocked S3/Bedrock clients."""
    processor = EmbeddingBatchProcessor(
        documents_repo=documents_repo,
        queue_repo=queue_repo,
        embeddings_repo=embeddings_repo,
        batch_jobs_repo=batch_jobs_repo,
        content_storage=AsyncMock(),
        embedding_provider=mock_embedding_provider,
        provider_type="bedrock",
    )
    # Mock S3 and Bedrock clients to avoid real AWS calls
    processor.storage_client = AsyncMock()
    processor.batch_provider = AsyncMock()
    return processor


# =============================================================================
# Online Processing Tests (Real DB)
# =============================================================================


@pytest.mark.integration
async def test_online_processes_document_end_to_end(
    db_pool, online_processor, queue_repo, embeddings_repo, documents_repo
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

    doc = await documents_repo.get_by_id(doc_id)
    assert doc.embedding_status == "completed"


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
# Bedrock Accumulation Tests (Real DB for queue counts)
# =============================================================================


@pytest.mark.integration
async def test_accumulation_skips_empty_queue(db_pool, bedrock_processor):
    """No batch created when queue is empty."""
    await bedrock_processor._check_and_create_batch()

    async with db_pool.acquire() as conn:
        jobs = await conn.fetch("SELECT * FROM embedding_batch_jobs")
        assert len(jobs) == 0


# =============================================================================
# Bedrock Output Parsing Tests (Unit - no DB needed)
# =============================================================================


@pytest.mark.unit
def test_parse_bedrock_output_groups_by_document():
    """Verify Bedrock JSONL output is correctly parsed and grouped."""
    # Create minimal processor for parsing test
    processor = EmbeddingBatchProcessor(
        documents_repo=None,
        queue_repo=None,
        embeddings_repo=None,
        batch_jobs_repo=None,
        content_storage=None,
        embedding_provider=None,
        provider_type="jina",  # Avoid Bedrock client init
    )

    output_lines = [
        {"recordId": "doc1:0:0:100", "modelOutput": {"embedding": [0.1] * 1024}},
        {"recordId": "doc1:1:100:200", "modelOutput": {"embedding": [0.2] * 1024}},
        {"recordId": "doc2:0:0:50", "modelOutput": {"embedding": [0.3] * 1024}},
    ]

    result = processor._parse_bedrock_output(output_lines)

    assert "doc1" in result
    assert "doc2" in result
    assert len(result["doc1"]) == 2
    assert len(result["doc2"]) == 1

    # Verify sorted by chunk_index
    assert result["doc1"][0]["chunk_index"] == 0
    assert result["doc1"][1]["chunk_index"] == 1


@pytest.mark.unit
def test_parse_bedrock_output_skips_errors():
    """Error records in Bedrock output are skipped gracefully."""
    processor = EmbeddingBatchProcessor(
        documents_repo=None,
        queue_repo=None,
        embeddings_repo=None,
        batch_jobs_repo=None,
        content_storage=None,
        embedding_provider=None,
        provider_type="jina",
    )

    output_lines = [
        {"recordId": "doc1:0:0:100", "error": {"message": "Rate limit"}},
        {"recordId": "doc1:1:100:200", "modelOutput": {"embedding": [0.1] * 1024}},
    ]

    result = processor._parse_bedrock_output(output_lines)

    assert len(result["doc1"]) == 1
    assert result["doc1"][0]["chunk_index"] == 1


# =============================================================================
# Large Document Handling Tests
# =============================================================================


@pytest.fixture
async def online_processor_with_sliding_window(
    db_pool,
    documents_repo,
    queue_repo,
    embeddings_repo,
    batch_jobs_repo,
):
    """Processor with a mock embedding provider that tracks calls for large doc testing."""

    content_storage = AsyncMock()

    async def get_text_from_db(content_id):
        async with db_pool.acquire() as conn:
            row = await conn.fetchrow(
                "SELECT content FROM content_blobs WHERE id = $1", content_id
            )
            return row["content"].decode() if row else None

    content_storage.get_text = get_text_from_db

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
        batch_jobs_repo=batch_jobs_repo,
        content_storage=content_storage,
        embedding_provider=provider,
        provider_type="jina",
    )


@pytest.mark.integration
async def test_online_processes_large_document_with_sliding_window(
    db_pool,
    online_processor_with_sliding_window,
    queue_repo,
    embeddings_repo,
    documents_repo,
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

    doc = await documents_repo.get_by_id(doc_id)
    assert doc.embedding_status == "completed"


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
