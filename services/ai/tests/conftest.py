"""Shared pytest fixtures for omni-ai tests.

This module provides fixtures for integration testing with real Postgres and Redis,
as well as mock fixtures for unit testing.

Test Strategy:
- Integration tests: Use real Postgres/Redis via testcontainers
- Unit tests: Use mocked providers and in-memory state
- LLM/Embedding APIs: Always mocked via respx to avoid API costs
"""

import os

# Set required env vars before importing app modules (config.py exits if these are missing)
os.environ.setdefault("PORT", "8000")
os.environ.setdefault("MODEL_PATH", "/tmp/models")
os.environ.setdefault("REDIS_URL", "redis://localhost:6379")
os.environ.setdefault("DATABASE_HOST", "localhost")
os.environ.setdefault("DATABASE_USERNAME", "test")
os.environ.setdefault("DATABASE_PASSWORD", "test")
os.environ.setdefault("DATABASE_NAME", "test")
os.environ.setdefault("CONNECTOR_MANAGER_URL", "http://localhost:9090")
os.environ.setdefault("AWS_REGION", "us-east-1")

import asyncio
import subprocess
from pathlib import Path
from typing import AsyncGenerator
from unittest.mock import AsyncMock, MagicMock

import asyncpg
import httpx
import pytest
import redis.asyncio as aioredis
import respx
from fastapi import FastAPI
from httpx import ASGITransport, AsyncClient
from testcontainers.core.container import DockerContainer
from testcontainers.core.waiting_utils import wait_for_logs
from testcontainers.redis import RedisContainer

from db import (
    DocumentsRepository,
    EmbeddingQueueRepository,
    EmbeddingsRepository,
)
from routers import chat_router, embeddings_router, health_router, prompts_router
from services import EmbeddingQueueService
from state import AppState


@pytest.fixture(scope="session")
def event_loop():
    """Create event loop for async tests."""
    policy = asyncio.get_event_loop_policy()
    loop = policy.new_event_loop()
    yield loop
    loop.close()


# =============================================================================
# Database Fixtures (for integration tests)
# =============================================================================


@pytest.fixture(scope="session")
def migrator_image():
    """Build the migrator image used by integration tests."""
    repo_root = Path(__file__).resolve().parents[3]
    image_tag = "omni-migrator:test"
    result = subprocess.run(
        [
            "docker",
            "build",
            "-f",
            "services/migrations/Dockerfile",
            "-t",
            image_tag,
            ".",
        ],
        cwd=str(repo_root),
        capture_output=True,
        text=True,
        timeout=900,
    )
    if result.returncode != 0:
        pytest.skip(f"Failed to build migrator image: {result.stderr[-500:]}")
    return image_tag


@pytest.fixture(scope="session")
def postgres_container():
    """Start ParadeDB PostgreSQL container for integration tests."""
    import time

    container = (
        DockerContainer("paradedb/paradedb:0.24.0-pg17")
        .with_exposed_ports(5432)
        .with_env("POSTGRES_USER", "test")
        .with_env("POSTGRES_PASSWORD", "test")
        .with_env("POSTGRES_DB", "test")
    )
    with container:
        wait_for_logs(container, "database system is ready to accept connections")
        # Give ParadeDB extra time to initialize extensions
        time.sleep(3)
        yield container


def _get_postgres_url(container) -> str:
    """Get PostgreSQL connection URL from container."""
    host = container.get_container_host_ip()
    port = container.get_exposed_port(5432)
    return f"postgresql://test:test@{host}:{port}/test"


@pytest.fixture(scope="session")
def initialized_db(postgres_container, migrator_image):
    """Run migrations through the same migrator image used in deployments."""
    pg_port = postgres_container.get_exposed_port(5432)
    last_logs = (b"", b"")
    for attempt in range(5):
        migrator = (
            DockerContainer(migrator_image)
            .with_env("DATABASE_HOST", "host.docker.internal")
            .with_env("DATABASE_PORT", str(pg_port))
            .with_env("DATABASE_USERNAME", "test")
            .with_env("DATABASE_PASSWORD", "test")
            .with_env("DATABASE_NAME", "test")
            .with_env("DATABASE_SSL", "false")
        )
        migrator._kwargs = {"extra_hosts": {"host.docker.internal": "host-gateway"}}
        migrator.start()
        try:
            result = migrator._container.wait(timeout=120)
            status_code = result.get("StatusCode", 1)
            if status_code == 0:
                break
            last_logs = migrator.get_logs()
        finally:
            migrator.stop()
        if attempt < 4:
            import time

            time.sleep(2)
    else:
        stdout, stderr = last_logs
        pytest.fail(
            "Migrator failed after retries.\n" f"stdout:\n{stdout}\nstderr:\n{stderr}"
        )
    yield postgres_container


@pytest.fixture
async def db_pool(initialized_db) -> AsyncGenerator:
    """Create async connection pool to initialized test database."""
    from pgvector.asyncpg import register_vector

    url = _get_postgres_url(initialized_db)

    async def init_connection(conn):
        await register_vector(conn)

    pool = await asyncpg.create_pool(
        url, min_size=2, max_size=10, ssl=False, init=init_connection
    )
    try:
        yield pool
    finally:
        await pool.close()


@pytest.fixture
async def db_session(db_pool) -> AsyncGenerator:
    """Create a database session with transaction rollback for test isolation."""
    async with db_pool.acquire() as conn:
        tr = conn.transaction()
        await tr.start()
        try:
            yield conn
        finally:
            await tr.rollback()


# =============================================================================
# Repository Fixtures (for integration tests)
# =============================================================================


@pytest.fixture
def documents_repo(db_pool):
    """DocumentsRepository with real database connection."""
    return DocumentsRepository(db_pool)


@pytest.fixture
def queue_repo(db_pool):
    """EmbeddingQueueRepository with real database connection."""
    return EmbeddingQueueRepository(db_pool)


@pytest.fixture
def embeddings_repo(db_pool):
    """EmbeddingsRepository with real database connection."""
    return EmbeddingsRepository(db_pool)


# =============================================================================
# Cache Fixtures (for integration tests)
# =============================================================================


@pytest.fixture(scope="session")
def redis_container():
    """Start Redis container for integration tests."""
    with RedisContainer("redis:7-alpine") as redis:
        yield redis


@pytest.fixture
async def redis_client(redis_container) -> AsyncGenerator:
    """Create async Redis client for tests."""
    host = redis_container.get_container_host_ip()
    port = redis_container.get_exposed_port(6379)

    client = aioredis.Redis(host=host, port=int(port), decode_responses=True)
    try:
        await client.flushdb()
        yield client
    finally:
        await client.close()


# =============================================================================
# Mock Fixtures (for unit tests)
# =============================================================================


@pytest.fixture
def mock_embedding_provider():
    """Mock embedding provider for unit tests."""
    provider = AsyncMock()
    # Use MagicMock for sync method to avoid coroutine return
    provider.get_model_name = MagicMock(return_value="test-embedding-model")

    mock_chunk = MagicMock()
    mock_chunk.span = (0, 100)
    mock_chunk.embedding = [0.1] * 1024

    provider.generate_embeddings.return_value = [mock_chunk]
    return provider


@pytest.fixture
def mock_llm_provider():
    """Mock LLM provider for unit tests."""
    provider = AsyncMock()
    provider.health_check.return_value = True
    from providers import TokenUsage

    provider.generate_response.return_value = (
        "This is a test response from the mock LLM.",
        TokenUsage(input_tokens=10, output_tokens=20),
    )

    async def mock_stream(*args, **kwargs):
        mock_event = MagicMock()
        mock_event.type = "content_block_delta"
        mock_event.delta.text = "Streamed response"
        yield mock_event

    provider.stream_response = mock_stream
    return provider


@pytest.fixture
def mock_jina_api():
    """Mock Jina embedding API using respx."""
    with respx.mock:
        respx.post("https://api.jina.ai/v1/embeddings").mock(
            return_value=httpx.Response(
                200,
                json={
                    "data": [{"embedding": [0.1] * 1024}],
                    "model": "jina-embeddings-v3",
                    "usage": {"total_tokens": 10},
                },
            )
        )
        yield


@pytest.fixture
def mock_openai_api():
    """Mock OpenAI embedding API using respx."""
    with respx.mock:
        respx.post("https://api.openai.com/v1/embeddings").mock(
            return_value=httpx.Response(
                200,
                json={
                    "data": [{"embedding": [0.1] * 1024, "index": 0}],
                    "model": "text-embedding-3-small",
                    "usage": {"prompt_tokens": 10, "total_tokens": 10},
                },
            )
        )
        yield


# =============================================================================
# App Fixtures (for endpoint tests)
# =============================================================================


@pytest.fixture
def app_state(mock_embedding_provider, mock_llm_provider):
    """Create AppState with mocked providers for unit tests."""
    state = AppState()
    state.embedding_provider = mock_embedding_provider
    state.models = {"mock-model": mock_llm_provider}
    state.default_model_id = "mock-model"
    state.searcher_tool = AsyncMock()
    state.content_storage = AsyncMock()
    return state


@pytest.fixture
def test_app(app_state, mock_embedding_provider):
    """Create test FastAPI application with mocked state."""
    from schemas import EmbeddingResponse

    app = FastAPI(title="Omni AI Service Test")
    app.state = app_state

    app.include_router(health_router)
    app.include_router(embeddings_router)
    app.include_router(prompts_router)
    app.include_router(chat_router)

    # Mock embedding queue to return EmbeddingResponse directly
    embedding_queue = AsyncMock(spec=EmbeddingQueueService)

    async def mock_enqueue(body, request_id):
        """Mock enqueue that returns a future resolving to EmbeddingResponse."""
        import asyncio

        # Generate proper response based on input texts
        embeddings = []
        chunks_spans = []
        chunks_count = []

        for text in body.texts:
            # Generate a single chunk per text with correct span
            text_embeddings = [[0.1] * 1024]  # One embedding per text
            text_spans = [(0, len(text))]  # Span covers full text
            embeddings.append(text_embeddings)
            chunks_spans.append(text_spans)
            chunks_count.append(1)

        # Return a future that immediately resolves
        future = asyncio.Future()
        future.set_result(
            EmbeddingResponse(
                embeddings=embeddings,
                chunks=chunks_spans,
                chunks_count=chunks_count,
                model_name=mock_embedding_provider.get_model_name(),
            )
        )
        return future

    embedding_queue.enqueue = mock_enqueue
    app.state.embedding_queue = embedding_queue

    return app


@pytest.fixture
async def async_client(test_app) -> AsyncGenerator:
    """Async HTTP client for testing endpoints."""
    async with AsyncClient(
        transport=ASGITransport(app=test_app), base_url="http://test"
    ) as client:
        yield client
