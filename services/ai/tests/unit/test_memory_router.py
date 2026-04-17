"""Unit tests for the /memories proxy router.

These verify session-scoping: the x-user-id header must be present, and
a user must not be able to delete a memory they do not own.
"""
import pytest
from fastapi import FastAPI
from fastapi.testclient import TestClient

from routers.memory import router as memory_router


class _FakeMemoryClient:
    """In-memory stand-in for MemoryClient that scopes by user_id."""

    def __init__(self):
        self._store: dict[str, list[dict]] = {}
        self.delete_calls: list[str] = []
        self.delete_all_calls: list[str] = []

    def seed(self, user_id: str, memories: list[dict]) -> None:
        self._store[user_id] = list(memories)

    async def list(self, user_id: str) -> list[dict]:
        return list(self._store.get(user_id, []))

    async def delete(self, memory_id: str) -> bool:
        self.delete_calls.append(memory_id)
        for mems in self._store.values():
            for i, m in enumerate(mems):
                if m.get("id") == memory_id:
                    mems.pop(i)
                    return True
        return False

    async def delete_all(self, user_id: str) -> bool:
        self.delete_all_calls.append(user_id)
        self._store.pop(user_id, None)
        return True


def _build_app(memory_client: _FakeMemoryClient | None) -> TestClient:
    app = FastAPI()
    app.include_router(memory_router)

    class _State:
        pass

    state = _State()
    if memory_client is not None:
        state.memory_client = memory_client
    app.state = state
    return TestClient(app)


@pytest.mark.unit
class TestMemoryRouter:
    def test_list_requires_user_id_header(self):
        client = _build_app(_FakeMemoryClient())
        resp = client.get("/memories")
        assert resp.status_code == 401

    def test_list_returns_503_when_client_not_configured(self):
        client = _build_app(None)
        resp = client.get("/memories", headers={"x-user-id": "alice"})
        assert resp.status_code == 503

    def test_list_returns_only_callers_memories(self):
        mem = _FakeMemoryClient()
        mem.seed("alice", [{"id": "m1", "memory": "alice secret"}])
        mem.seed("bob", [{"id": "m2", "memory": "bob secret"}])
        client = _build_app(mem)

        resp = client.get("/memories", headers={"x-user-id": "alice"})
        assert resp.status_code == 200
        data = resp.json()
        assert [m["id"] for m in data["memories"]] == ["m1"]

    def test_delete_one_rejects_non_owned_id(self):
        """User A cannot delete user B's memory — must return 404 and not call delete."""
        mem = _FakeMemoryClient()
        mem.seed("alice", [{"id": "m1", "memory": "alice"}])
        mem.seed("bob", [{"id": "m2", "memory": "bob"}])
        client = _build_app(mem)

        resp = client.delete("/memories/m2", headers={"x-user-id": "alice"})
        assert resp.status_code == 404
        # mem0 was never asked to delete — protect against leak-by-existence.
        assert mem.delete_calls == []

    def test_delete_one_succeeds_for_owned_id(self):
        mem = _FakeMemoryClient()
        mem.seed("alice", [{"id": "m1", "memory": "alice"}])
        client = _build_app(mem)

        resp = client.delete("/memories/m1", headers={"x-user-id": "alice"})
        assert resp.status_code == 200
        assert mem.delete_calls == ["m1"]

    def test_delete_all_passes_callers_user_id(self):
        mem = _FakeMemoryClient()
        mem.seed("alice", [{"id": "m1", "memory": "alice"}])
        mem.seed("bob", [{"id": "m2", "memory": "bob"}])
        client = _build_app(mem)

        resp = client.delete("/memories", headers={"x-user-id": "alice"})
        assert resp.status_code == 200
        assert mem.delete_all_calls == ["alice"]
        # Bob's memories are untouched.
        assert any(m["id"] == "m2" for m in mem._store.get("bob", []))

    def test_delete_org_agent_requires_admin_role(self):
        mem = _FakeMemoryClient()
        mem.seed("org_agent:agent-123", [{"id": "m1", "memory": "org secret"}])
        client = _build_app(mem)

        # Missing role header → 403
        resp = client.delete(
            "/memories/org-agent/agent-123",
            headers={"x-user-id": "alice"},
        )
        assert resp.status_code == 403
        # Non-admin role → 403
        resp = client.delete(
            "/memories/org-agent/agent-123",
            headers={"x-user-id": "alice", "x-user-role": "user"},
        )
        assert resp.status_code == 403
        assert mem.delete_all_calls == []

    def test_delete_org_agent_purges_namespace_for_admin(self):
        mem = _FakeMemoryClient()
        mem.seed("org_agent:agent-123", [{"id": "m1", "memory": "org secret"}])
        mem.seed("alice", [{"id": "m2", "memory": "alice"}])
        client = _build_app(mem)

        resp = client.delete(
            "/memories/org-agent/agent-123",
            headers={"x-user-id": "admin-user", "x-user-role": "admin"},
        )
        assert resp.status_code == 200
        assert mem.delete_all_calls == ["org_agent:agent-123"]
        # Unrelated namespace untouched.
        assert mem._store.get("alice")

    def test_delete_org_agent_returns_503_when_client_missing(self):
        client = _build_app(None)
        resp = client.delete(
            "/memories/org-agent/agent-123",
            headers={"x-user-id": "admin", "x-user-role": "admin"},
        )
        assert resp.status_code == 503
