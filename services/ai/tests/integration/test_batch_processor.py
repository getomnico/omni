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
async def test_online_processes_document_end_to_end(db_pool, online_processor):
    """Full flow: queue item -> fetch -> embed -> store in DB -> mark complete."""
    # Setup: create test data in real DB
    user_id = await create_test_user(db_pool)
    source_id = await create_test_source(db_pool, user_id)
    doc_id = await create_test_document(
        db_pool, source_id, "Test content for embedding."
    )
    queue_id = await enqueue_document(db_pool, doc_id)

    # Process the batch
    await online_processor._process_online_batch()

    # Verify: embeddings stored in DB
    async with db_pool.acquire() as conn:
        embeddings = await conn.fetch(
            "SELECT * FROM embeddings WHERE document_id = $1", doc_id
        )
        assert len(embeddings) >= 1
        assert len(embeddings[0]["embedding"]) == 1024  # Vector dimension

        # Verify: queue item marked completed
        queue_item = await conn.fetchrow(
            "SELECT status FROM embedding_queue WHERE id = $1", queue_id
        )
        assert queue_item["status"] == "completed"

        # Verify: document embedding_status updated
        doc = await conn.fetchrow(
            "SELECT embedding_status FROM documents WHERE id = $1", doc_id
        )
        assert doc["embedding_status"] == "completed"


@pytest.mark.integration
async def test_online_handles_empty_content(db_pool, online_processor):
    """Empty document content marks queue item as failed."""
    user_id = await create_test_user(db_pool)
    source_id = await create_test_source(db_pool, user_id)
    doc_id = await create_test_document(db_pool, source_id, "")  # Empty content
    queue_id = await enqueue_document(db_pool, doc_id)

    await online_processor._process_online_batch()

    async with db_pool.acquire() as conn:
        queue_item = await conn.fetchrow(
            "SELECT status, error_message FROM embedding_queue WHERE id = $1", queue_id
        )
        assert queue_item["status"] == "failed"
        assert queue_item["error_message"] is not None

        # No embeddings should be created
        embeddings = await conn.fetch(
            "SELECT * FROM embeddings WHERE document_id = $1", doc_id
        )
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
async def online_processor_with_char_chunking(
    db_pool,
    documents_repo,
    queue_repo,
    embeddings_repo,
    batch_jobs_repo,
):
    """Processor with a mock embedding provider that tracks calls for large doc testing."""
    from unittest.mock import AsyncMock, MagicMock, call

    # Mock content storage to fetch from DB
    content_storage = AsyncMock()

    async def get_text_from_db(content_id):
        async with db_pool.acquire() as conn:
            row = await conn.fetchrow(
                "SELECT content FROM content_blobs WHERE id = $1", content_id
            )
            return row["content"].decode() if row else None

    content_storage.get_text = get_text_from_db

    # Create mock provider that returns chunks with correct spans
    provider = AsyncMock()
    provider.get_model_name = MagicMock(return_value="test-embedding-model")

    async def generate_with_spans(text, **kwargs):
        """Generate embeddings with spans matching the input text."""
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
async def test_online_processes_large_document_with_char_chunking(
    db_pool, online_processor_with_char_chunking, monkeypatch
):
    """Large documents exceeding size limit are split by characters and still processed."""
    import embeddings.batch_processor as bp

    # Set a small limit for testing (100 bytes)
    monkeypatch.setattr(bp, "EMBEDDING_MAX_DOCUMENT_SIZE", 100)

    user_id = await create_test_user(db_pool)
    source_id = await create_test_source(db_pool, user_id)

    # Create document with content larger than 100 bytes
    large_content = "This is a test sentence. " * 20  # ~500 bytes
    doc_id = await create_test_document(db_pool, source_id, large_content)
    queue_id = await enqueue_document(db_pool, doc_id)

    await online_processor_with_char_chunking._process_online_batch()

    async with db_pool.acquire() as conn:
        # Verify: queue item marked completed (not failed)
        queue_item = await conn.fetchrow(
            "SELECT status FROM embedding_queue WHERE id = $1", queue_id
        )
        assert queue_item["status"] == "completed"

        # Verify: embeddings were created
        embeddings = await conn.fetch(
            "SELECT * FROM embeddings WHERE document_id = $1 ORDER BY chunk_index",
            doc_id,
        )
        assert len(embeddings) >= 1

        # Verify: spans cover the full document
        first_span_start = embeddings[0]["chunk_start_offset"]
        last_span_end = embeddings[-1]["chunk_end_offset"]
        assert first_span_start == 0
        assert last_span_end == len(large_content)
