"""Integration tests for the memory service HTTP API.

Tests mock the mem0.Memory instance and psycopg/sqlite3 I/O so no real
database or LLM is required.  The server lifespan still runs for every
test (reads /tmp/mem0_config.json, sets module globals) — keeping the
startup path exercised without the heavy external dependencies.
"""

import json
from unittest.mock import MagicMock, patch

import pytest
from httpx import ASGITransport, AsyncClient

# Written once per session so the server lifespan can open it.
_TEST_DB_CONFIG = {
    "host": "localhost",
    "port": 5432,
    "dbname": "test_mem0",
    "user": "test",
    "password": "test",
    "collection_name": "mem0_memories_test",
}

_TEST_CONFIG = {
    "vector_store": {"provider": "pgvector", "config": _TEST_DB_CONFIG},
    "llm": {"provider": "openai", "config": {"model": "gpt-4o-mini"}},
    "embedder": {
        "provider": "openai",
        "config": {"model": "text-embedding-3-small"},
    },
    "history_db_path": "/tmp/mem0_history_test.db",
}


# ─── Fixtures ─────────────────────────────────────────────────────────────────


@pytest.fixture(scope="session", autouse=True)
def write_test_config():
    """Write a minimal mem0 config so the server lifespan can read it."""
    with open("/tmp/mem0_config.json", "w") as f:
        json.dump(_TEST_CONFIG, f)


@pytest.fixture
def mock_memory():
    m = MagicMock()
    m.db.db_path = "/tmp/mem0_history_test.db"
    return m


@pytest.fixture
async def client(mock_memory):
    """AsyncClient wired to the server app with mem0 replaced by a MagicMock.

    Patching server._load_memory makes the lifespan assign mock_memory to the
    server._memory global, so every endpoint call uses the mock.
    """
    import server

    with patch("server._load_memory", return_value=mock_memory):
        async with server.lifespan(server.app):
            async with AsyncClient(
                transport=ASGITransport(app=server.app), base_url="http://test"
            ) as c:
                yield c, mock_memory


# ─── GET /health ──────────────────────────────────────────────────────────────


@pytest.mark.integration
async def test_health_ok(client):
    c, _ = client
    with patch("psycopg.connect") as mock_connect:
        mock_connect.return_value.close = MagicMock()
        response = await c.get("/health")
    assert response.status_code == 200
    assert response.json() == {"status": "ok"}


@pytest.mark.integration
async def test_health_db_failure_returns_503(client):
    c, _ = client
    with patch("psycopg.connect", side_effect=Exception("connection refused")):
        response = await c.get("/health")
    assert response.status_code == 503
    body = response.json()
    assert body["status"] == "unhealthy"
    assert "connection refused" in body["reason"]


# ─── POST /memories ───────────────────────────────────────────────────────────


@pytest.mark.integration
async def test_add_memories_returns_mem0_response(client):
    c, mock_mem = client
    mock_mem.add.return_value = {"results": [{"id": "mem-1", "memory": "prefers dark mode"}]}
    messages = [{"role": "user", "content": "I prefer dark mode"}]

    response = await c.post("/memories", json={"messages": messages, "user_id": "u1"})

    assert response.status_code == 200
    assert response.json() == {"results": [{"id": "mem-1", "memory": "prefers dark mode"}]}
    mock_mem.add.assert_called_once_with(messages, user_id="u1")


@pytest.mark.integration
async def test_add_memories_all_empty_content_returns_empty_without_calling_mem0(client):
    c, mock_mem = client
    messages = [{"role": "user", "content": ""}]

    response = await c.post("/memories", json={"messages": messages, "user_id": "u1"})

    assert response.status_code == 200
    assert response.json() == {}
    mock_mem.add.assert_not_called()


@pytest.mark.integration
async def test_add_memories_flattens_list_content_to_text(client):
    """List-type content (vision messages) is collapsed to plain text before
    being passed to mem0, which doesn't handle list content without a vision LLM."""
    c, mock_mem = client
    mock_mem.add.return_value = {}
    messages = [
        {
            "role": "user",
            "content": [
                {"type": "text", "text": "Hello"},
                {"type": "image_url", "url": "http://img.example.com/x.png"},
                {"type": "text", "text": "world"},
            ],
        }
    ]

    await c.post("/memories", json={"messages": messages, "user_id": "u1"})

    added_messages = mock_mem.add.call_args[0][0]
    assert added_messages == [{"role": "user", "content": "Hello world"}]


@pytest.mark.integration
async def test_add_memories_non_text_only_list_filtered_out(client):
    """A list with only non-text blocks produces empty content and is skipped."""
    c, mock_mem = client
    messages = [{"role": "user", "content": [{"type": "image_url", "url": "http://img"}]}]

    response = await c.post("/memories", json={"messages": messages, "user_id": "u1"})

    assert response.json() == {}
    mock_mem.add.assert_not_called()


@pytest.mark.integration
async def test_add_memories_mixed_valid_and_empty_sends_only_valid(client):
    c, mock_mem = client
    mock_mem.add.return_value = {}
    messages = [
        {"role": "user", "content": "keep this"},
        {"role": "assistant", "content": ""},
        {"role": "user", "content": "and this"},
    ]

    await c.post("/memories", json={"messages": messages, "user_id": "u1"})

    added = mock_mem.add.call_args[0][0]
    assert len(added) == 2
    assert added[0]["content"] == "keep this"
    assert added[1]["content"] == "and this"


# ─── POST /search ─────────────────────────────────────────────────────────────


@pytest.mark.integration
async def test_search_wraps_list_result_in_results_key(client):
    c, mock_mem = client
    mock_mem.search.return_value = [{"id": "m1", "memory": "prefers dark mode", "score": 0.9}]

    response = await c.post(
        "/search", json={"query": "display preference", "user_id": "u1"}
    )

    assert response.status_code == 200
    assert response.json() == {
        "results": [{"id": "m1", "memory": "prefers dark mode", "score": 0.9}]
    }


@pytest.mark.integration
async def test_search_passes_user_filter_and_top_k_to_mem0(client):
    c, mock_mem = client
    mock_mem.search.return_value = []

    await c.post("/search", json={"query": "theme", "user_id": "alice", "top_k": 3})

    mock_mem.search.assert_called_once_with(
        "theme", top_k=3, filters={"user_id": "alice"}
    )


@pytest.mark.integration
async def test_search_default_top_k_is_five(client):
    c, mock_mem = client
    mock_mem.search.return_value = []

    await c.post("/search", json={"query": "q", "user_id": "u1"})

    mock_mem.search.assert_called_once_with("q", top_k=5, filters={"user_id": "u1"})


@pytest.mark.integration
async def test_search_returns_dict_directly_when_mem0_returns_dict(client):
    c, mock_mem = client
    mock_mem.search.return_value = {"results": [], "total": 0, "page": 1}

    response = await c.post("/search", json={"query": "q", "user_id": "u1"})

    assert response.json() == {"results": [], "total": 0, "page": 1}


# ─── GET /memories ────────────────────────────────────────────────────────────


@pytest.mark.integration
async def test_list_memories_wraps_list_in_results_key(client):
    c, mock_mem = client
    mock_mem.get_all.return_value = [{"id": "m1", "memory": "night owl"}]

    response = await c.get("/memories?user_id=u1")

    assert response.status_code == 200
    assert response.json() == {"results": [{"id": "m1", "memory": "night owl"}]}
    mock_mem.get_all.assert_called_once_with(filters={"user_id": "u1"})


@pytest.mark.integration
async def test_list_memories_returns_dict_directly_when_mem0_returns_dict(client):
    c, mock_mem = client
    mock_mem.get_all.return_value = {"results": [], "page": 1}

    response = await c.get("/memories?user_id=u1")

    assert response.json() == {"results": [], "page": 1}


# ─── DELETE /memories/{memory_id} ─────────────────────────────────────────────


@pytest.mark.integration
async def test_delete_single_memory_returns_deleted(client):
    c, mock_mem = client

    response = await c.delete("/memories/mem-abc-123")

    assert response.status_code == 200
    assert response.json() == {"status": "deleted"}
    mock_mem.delete.assert_called_once_with("mem-abc-123")


# ─── DELETE /memories ─────────────────────────────────────────────────────────


def _make_pg_mock(tables: list[str], rows_per_table: int = 2) -> MagicMock:
    """Build a psycopg context-manager mock for _purge_user_across_all_collections."""
    conn = MagicMock()
    conn.__enter__ = lambda s: conn
    conn.__exit__ = MagicMock(return_value=False)
    # fetchall returns table names; rowcount is used for DELETE results
    conn.execute.return_value.fetchall.return_value = [(t,) for t in tables]
    conn.execute.return_value.rowcount = rows_per_table
    return conn


def _make_sqlite_mock() -> MagicMock:
    conn = MagicMock()
    conn.__enter__ = lambda s: conn
    conn.__exit__ = MagicMock(return_value=False)
    return conn


@pytest.mark.integration
async def test_delete_all_memories_returns_status_and_row_count(client):
    c, mock_mem = client
    pg_mock = _make_pg_mock(tables=["mem0_memories_abc"], rows_per_table=3)
    sqlite_mock = _make_sqlite_mock()

    with (
        patch("sqlite3.connect", return_value=sqlite_mock),
        patch("psycopg.connect", return_value=pg_mock),
    ):
        response = await c.delete("/memories?user_id=u1")

    assert response.status_code == 200
    data = response.json()
    assert data["status"] == "deleted"
    assert data["rows_deleted"] == 3
    mock_mem.delete_all.assert_called_once_with(user_id="u1")


@pytest.mark.integration
async def test_delete_all_clears_sqlite_history_for_user(client):
    c, mock_mem = client
    mock_mem.db.db_path = "/tmp/mem0_history_test.db"
    pg_mock = _make_pg_mock(tables=[])
    sqlite_mock = _make_sqlite_mock()

    with (
        patch("sqlite3.connect", return_value=sqlite_mock) as sqlite_spy,
        patch("psycopg.connect", return_value=pg_mock),
    ):
        await c.delete("/memories?user_id=u2")

    sqlite_spy.assert_called_once_with("/tmp/mem0_history_test.db")
    sqlite_mock.execute.assert_called_once_with(
        "DELETE FROM messages WHERE session_scope = ?",
        ("user_id=u2",),
    )


@pytest.mark.integration
async def test_delete_all_purges_across_multiple_collections(client):
    c, _ = client
    pg_mock = _make_pg_mock(
        tables=["mem0_memories_aaa", "mem0_memories_bbb"], rows_per_table=1
    )
    sqlite_mock = _make_sqlite_mock()

    with (
        patch("sqlite3.connect", return_value=sqlite_mock),
        patch("psycopg.connect", return_value=pg_mock),
    ):
        response = await c.delete("/memories?user_id=u3")

    # 2 tables × 1 row each = 2 rows deleted
    assert response.json()["rows_deleted"] == 2


@pytest.mark.integration
async def test_delete_all_rows_deleted_zero_when_no_collections(client):
    c, _ = client
    pg_mock = _make_pg_mock(tables=[])
    sqlite_mock = _make_sqlite_mock()

    with (
        patch("sqlite3.connect", return_value=sqlite_mock),
        patch("psycopg.connect", return_value=pg_mock),
    ):
        response = await c.delete("/memories?user_id=u4")

    assert response.json()["rows_deleted"] == 0
