"""Unit tests for Mem0Provider — the mem0-backed MemoryProvider impl."""

from unittest.mock import AsyncMock, MagicMock, patch

import pytest

from memory.providers.mem0.provider import Mem0Provider


class _FakePool:
    """Async-context pool stub. `pool.acquire()` yields the supplied conn."""

    def __init__(self, conn):
        self._conn = conn
        self.acquire_calls = 0

    def acquire(self):
        self.acquire_calls += 1
        return self

    async def __aenter__(self):
        if isinstance(self._conn, Exception):
            raise self._conn
        return self._conn

    async def __aexit__(self, *args):
        return None


def _provider(memory=None, pool=None):
    mem = memory or MagicMock()
    mem.db.db_path = "/tmp/mem0_history_test.db"
    pool = pool or _FakePool(MagicMock())
    return Mem0Provider(mem, pool), mem


@pytest.mark.unit
class TestAdd:
    @pytest.mark.asyncio
    async def test_add_empty_messages_returns_without_calling_mem0(self):
        svc, mem = _provider()
        await svc.add(messages=[], key="u1")
        mem.add.assert_not_called()

    @pytest.mark.asyncio
    async def test_add_flattens_list_content_to_text_only(self):
        svc, mem = _provider()
        messages = [
            {
                "role": "user",
                "content": [
                    {"type": "text", "text": "hello"},
                    {"type": "image", "url": "x"},
                    {"type": "text", "text": "world"},
                ],
            }
        ]
        await svc.add(messages=messages, key="u1")
        call_args = mem.add.call_args
        assert call_args.args[0] == [{"role": "user", "content": "hello world"}]
        assert call_args.kwargs == {"user_id": "u1"}

    @pytest.mark.asyncio
    async def test_add_drops_messages_that_collapse_to_empty(self):
        svc, mem = _provider()
        messages = [
            {"role": "user", "content": [{"type": "image", "url": "x"}]},
            {"role": "assistant", "content": "reply"},
        ]
        await svc.add(messages=messages, key="u1")
        assert mem.add.call_args.args[0] == [{"role": "assistant", "content": "reply"}]

    @pytest.mark.asyncio
    async def test_add_noop_when_entire_batch_collapses(self):
        svc, mem = _provider()
        await svc.add(
            messages=[{"role": "user", "content": [{"type": "image", "url": "x"}]}],
            key="u1",
        )
        mem.add.assert_not_called()

    @pytest.mark.asyncio
    async def test_add_swallows_mem0_errors(self):
        svc, mem = _provider()
        mem.add.side_effect = RuntimeError("mem0 down")
        await svc.add(messages=[{"role": "user", "content": "hi"}], key="u1")


@pytest.mark.unit
class TestSearch:
    @pytest.mark.asyncio
    async def test_search_returns_hits_with_score_and_record(self):
        svc, mem = _provider()
        mem.search.return_value = {
            "results": [
                {"id": "a", "memory": "fact a", "score": 0.9},
                {"id": "b", "memory": "fact b", "score": 0.5},
            ]
        }
        hits = await svc.search(query="q", key="u1", limit=5)
        assert [h.record.text for h in hits] == ["fact a", "fact b"]
        assert [h.record.id for h in hits] == ["a", "b"]
        assert [h.record.key for h in hits] == ["u1", "u1"]
        assert hits[0].score == 0.9
        mem.search.assert_called_once_with("q", top_k=5, filters={"user_id": "u1"})

    @pytest.mark.asyncio
    async def test_search_handles_bare_list_response(self):
        svc, mem = _provider()
        mem.search.return_value = [{"id": "a", "memory": "x", "score": 0.1}]
        hits = await svc.search(query="q", key="u1", limit=3)
        assert len(hits) == 1
        assert hits[0].record.text == "x"

    @pytest.mark.asyncio
    async def test_search_returns_empty_on_error(self):
        svc, mem = _provider()
        mem.search.side_effect = RuntimeError("down")
        assert await svc.search(query="q", key="u1", limit=5) == []

    @pytest.mark.asyncio
    async def test_search_defaults_score_to_zero_when_missing(self):
        svc, mem = _provider()
        mem.search.return_value = {"results": [{"id": "a", "memory": "x"}]}
        hits = await svc.search(query="q", key="u1", limit=5)
        assert hits[0].score == 0.0


@pytest.mark.unit
class TestListAndDelete:
    @pytest.mark.asyncio
    async def test_list_returns_records(self):
        svc, mem = _provider()
        mem.get_all.return_value = {"results": [{"id": "a", "memory": "m"}]}
        got = await svc.list(key="u1")
        assert len(got) == 1
        assert got[0].id == "a"
        assert got[0].text == "m"
        assert got[0].key == "u1"
        mem.get_all.assert_called_once_with(filters={"user_id": "u1"})

    @pytest.mark.asyncio
    async def test_list_normalises_bare_list(self):
        svc, mem = _provider()
        mem.get_all.return_value = [{"id": "a", "memory": "m"}]
        got = await svc.list(key="u1")
        assert [r.id for r in got] == ["a"]

    @pytest.mark.asyncio
    async def test_list_returns_empty_on_error(self):
        svc, mem = _provider()
        mem.get_all.side_effect = RuntimeError("down")
        assert await svc.list(key="u1") == []

    @pytest.mark.asyncio
    async def test_list_paginates_with_offset_and_limit(self):
        svc, mem = _provider()
        mem.get_all.return_value = {
            "results": [{"id": str(i), "memory": str(i)} for i in range(10)]
        }
        got = await svc.list(key="u1", limit=3, offset=2)
        assert [r.id for r in got] == ["2", "3", "4"]

    @pytest.mark.asyncio
    async def test_delete_returns_true_on_success(self):
        svc, mem = _provider()
        assert await svc.delete(memory_id="m1") is True
        mem.delete.assert_called_once_with("m1")

    @pytest.mark.asyncio
    async def test_delete_returns_false_on_error(self):
        svc, mem = _provider()
        mem.delete.side_effect = RuntimeError("down")
        assert await svc.delete(memory_id="m1") is False


def _async_pg_conn(rows: list[str], delete_count: int = 3) -> MagicMock:
    """Build a fake asyncpg connection with awaitable .fetch/.execute."""
    conn = MagicMock()
    conn.fetch = AsyncMock(return_value=[{"tablename": t} for t in rows])
    conn.execute = AsyncMock(return_value=f"DELETE {delete_count}")
    return conn


@pytest.mark.unit
class TestDeleteAll:
    @pytest.mark.asyncio
    async def test_delete_all_clears_sqlite_buffer_and_purges_tables(self):
        conn = _async_pg_conn(["mem0_memories_aaa", "mem0_memories_bbb"])
        pool = _FakePool(conn)
        svc, mem = _provider(pool=pool)

        sqlite_conn = MagicMock()
        sqlite_conn.__enter__ = MagicMock(return_value=sqlite_conn)
        sqlite_conn.__exit__ = MagicMock(return_value=False)

        with patch(
            "memory.providers.mem0.provider.sqlite3.connect",
            return_value=sqlite_conn,
        ):
            total = await svc.delete_all(key="u1")

        mem.delete_all.assert_called_once_with(user_id="u1")
        sqlite_conn.execute.assert_called_once_with(
            "DELETE FROM messages WHERE session_scope = ?",
            ("user_id=u1",),
        )
        # One acquire on the shared pool, one delete per table.
        assert pool.acquire_calls == 1
        assert conn.execute.await_count == 2
        assert total == 6  # 3 rows × 2 tables

    @pytest.mark.asyncio
    async def test_delete_all_returns_zero_on_purge_connect_failure(self):
        # Simulate the pool itself failing to hand out a connection.
        pool = _FakePool(RuntimeError("db down"))
        svc, mem = _provider(pool=pool)

        sqlite_conn = MagicMock()
        sqlite_conn.__enter__ = MagicMock(return_value=sqlite_conn)
        sqlite_conn.__exit__ = MagicMock(return_value=False)

        with patch(
            "memory.providers.mem0.provider.sqlite3.connect",
            return_value=sqlite_conn,
        ):
            total = await svc.delete_all(key="u1")
        assert total == 0
