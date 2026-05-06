"""Unit tests for the /memories proxy router.

These verify session-scoping: the x-user-id header must be present, and
a user must not be able to delete a memory they do not own.
"""

import pytest
from fastapi import FastAPI
from fastapi.testclient import TestClient

from memory import MemoryRecord, MemorySearchHit, agent_key, user_key
from routers.memory import router as memory_router


class _FakeMemoryProvider:
    """In-memory MemoryProvider stub that scopes by key."""

    def __init__(self):
        self._store: dict[str, list[MemoryRecord]] = {}
        self.delete_calls: list[str] = []
        self.delete_all_calls: list[str] = []
        self.add_calls: list[tuple[str, list]] = []

    def seed(self, key: str, memories: list[dict]) -> None:
        self._store[key] = [
            MemoryRecord(id=m["id"], text=m.get("memory", ""), key=key)
            for m in memories
        ]

    async def add(self, messages, key: str) -> None:
        self.add_calls.append((key, list(messages)))
        self._store.setdefault(key, []).append(
            MemoryRecord(
                id=f"seed-{len(self._store[key])}",
                text=messages[0].get("content", ""),
                key=key,
            )
        )

    async def search(self, query: str, key: str, limit: int = 5):
        records = self._store.get(key, [])[:limit]
        return [MemorySearchHit(record=r, score=1.0) for r in records]

    async def list(
        self, key: str, limit: int = 50, offset: int = 0
    ) -> list[MemoryRecord]:
        records = list(self._store.get(key, []))
        return records[offset : offset + limit]

    async def delete(self, memory_id: str) -> bool:
        self.delete_calls.append(memory_id)
        for records in self._store.values():
            for i, r in enumerate(records):
                if r.id == memory_id:
                    records.pop(i)
                    return True
        return False

    async def delete_all(self, key: str) -> int:
        self.delete_all_calls.append(key)
        purged = len(self._store.pop(key, []))
        return purged


def _build_app(memory_provider: _FakeMemoryProvider | None) -> TestClient:
    app = FastAPI()
    app.include_router(memory_router)

    class _State:
        memory_provider = None

    state = _State()
    state.memory_provider = memory_provider
    app.state = state
    return TestClient(app)


@pytest.mark.unit
class TestMemoryRouter:
    def test_list_requires_user_id_header(self):
        client = _build_app(_FakeMemoryProvider())
        resp = client.get("/memories")
        assert resp.status_code == 401

    def test_list_returns_503_when_provider_not_configured(self):
        client = _build_app(None)
        resp = client.get("/memories", headers={"x-user-id": "alice"})
        assert resp.status_code == 503

    def test_list_returns_only_callers_memories(self):
        mem = _FakeMemoryProvider()
        mem.seed(user_key("alice"), [{"id": "m1", "memory": "alice secret"}])
        mem.seed(user_key("bob"), [{"id": "m2", "memory": "bob secret"}])
        client = _build_app(mem)

        resp = client.get("/memories", headers={"x-user-id": "alice"})
        assert resp.status_code == 200
        data = resp.json()
        assert [m["id"] for m in data["memories"]] == ["m1"]
        assert data["memories"][0]["memory"] == "alice secret"

    def test_delete_one_rejects_non_owned_id(self):
        """User A cannot delete user B's memory — must return 404 and not call delete."""
        mem = _FakeMemoryProvider()
        mem.seed(user_key("alice"), [{"id": "m1", "memory": "alice"}])
        mem.seed(user_key("bob"), [{"id": "m2", "memory": "bob"}])
        client = _build_app(mem)

        resp = client.delete("/memories/m2", headers={"x-user-id": "alice"})
        assert resp.status_code == 404
        # Provider was never asked to delete — protect against leak-by-existence.
        assert mem.delete_calls == []

    def test_delete_one_succeeds_for_owned_id(self):
        mem = _FakeMemoryProvider()
        mem.seed(user_key("alice"), [{"id": "m1", "memory": "alice"}])
        client = _build_app(mem)

        resp = client.delete("/memories/m1", headers={"x-user-id": "alice"})
        assert resp.status_code == 200
        assert mem.delete_calls == ["m1"]

    def test_delete_all_passes_user_namespaced_key(self):
        mem = _FakeMemoryProvider()
        mem.seed(user_key("alice"), [{"id": "m1", "memory": "alice"}])
        mem.seed(user_key("bob"), [{"id": "m2", "memory": "bob"}])
        client = _build_app(mem)

        resp = client.delete("/memories", headers={"x-user-id": "alice"})
        assert resp.status_code == 200
        assert mem.delete_all_calls == [user_key("alice")]
        # Bob's memories are untouched.
        assert mem._store.get(user_key("bob"))

    def test_delete_agent_requires_admin_role(self):
        mem = _FakeMemoryProvider()
        mem.seed(agent_key("agent-123"), [{"id": "m1", "memory": "agent secret"}])
        client = _build_app(mem)

        # Missing role header → 403
        resp = client.delete(
            "/memories/agent/agent-123",
            headers={"x-user-id": "alice"},
        )
        assert resp.status_code == 403
        # Non-admin role → 403
        resp = client.delete(
            "/memories/agent/agent-123",
            headers={"x-user-id": "alice", "x-user-role": "user"},
        )
        assert resp.status_code == 403
        assert mem.delete_all_calls == []

    def test_delete_agent_purges_namespace_for_admin(self):
        mem = _FakeMemoryProvider()
        mem.seed(agent_key("agent-123"), [{"id": "m1", "memory": "agent secret"}])
        mem.seed(user_key("alice"), [{"id": "m2", "memory": "alice"}])
        client = _build_app(mem)

        resp = client.delete(
            "/memories/agent/agent-123",
            headers={"x-user-id": "admin-user", "x-user-role": "admin"},
        )
        assert resp.status_code == 200
        assert mem.delete_all_calls == [agent_key("agent-123")]
        assert mem._store.get(user_key("alice"))

    def test_delete_agent_returns_503_when_provider_missing(self):
        client = _build_app(None)
        resp = client.delete(
            "/memories/agent/agent-123",
            headers={"x-user-id": "admin", "x-user-role": "admin"},
        )
        assert resp.status_code == 503
