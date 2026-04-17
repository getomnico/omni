"""Unit tests for MemoryClient."""
import httpx
import pytest
import respx

from memory.client import MemoryClient


@pytest.mark.unit
class TestMemoryClient:
    @pytest.fixture
    def client(self):
        return MemoryClient(base_url="http://memory:8888")

    @respx.mock
    @pytest.mark.asyncio
    async def test_search_returns_facts(self, client):
        respx.post("http://memory:8888/search").mock(
            return_value=httpx.Response(
                200,
                json={"results": [{"memory": "User prefers bullet points"}]},
            )
        )
        facts = await client.search(query="formatting preferences", user_id="u1", limit=5)
        assert facts == ["User prefers bullet points"]

    @respx.mock
    @pytest.mark.asyncio
    async def test_search_returns_empty_on_error(self, client):
        respx.post("http://memory:8888/search").mock(
            return_value=httpx.Response(500, json={"error": "internal"})
        )
        facts = await client.search(query="anything", user_id="u1", limit=5)
        assert facts == []

    @respx.mock
    @pytest.mark.asyncio
    async def test_add_sends_messages_and_user_id(self, client):
        route = respx.post("http://memory:8888/memories").mock(
            return_value=httpx.Response(200, json={"id": "m1"})
        )
        messages = [
            {"role": "user", "content": "I like concise answers"},
            {"role": "assistant", "content": "Noted!"},
        ]
        await client.add(messages=messages, user_id="u1")
        assert route.called
        body = route.calls[0].request.read()
        import json
        payload = json.loads(body)
        assert payload["user_id"] == "u1"
        assert payload["messages"] == messages

    @respx.mock
    @pytest.mark.asyncio
    async def test_add_does_not_raise_on_error(self, client):
        respx.post("http://memory:8888/memories").mock(
            return_value=httpx.Response(500, json={"error": "internal"})
        )
        await client.add(
            messages=[{"role": "user", "content": "test"}], user_id="u1"
        )

    @respx.mock
    @pytest.mark.asyncio
    async def test_list_returns_results_from_dict_response(self, client):
        respx.get("http://memory:8888/memories").mock(
            return_value=httpx.Response(
                200,
                json={
                    "results": [
                        {"id": "m1", "memory": "fact one"},
                        {"id": "m2", "memory": "fact two"},
                    ]
                },
            )
        )
        memories = await client.list(user_id="u1")
        assert len(memories) == 2
        assert memories[0]["id"] == "m1"

    @respx.mock
    @pytest.mark.asyncio
    async def test_list_returns_bare_list_response(self, client):
        respx.get("http://memory:8888/memories").mock(
            return_value=httpx.Response(
                200, json=[{"id": "m1", "memory": "fact one"}]
            )
        )
        memories = await client.list(user_id="u1")
        assert memories == [{"id": "m1", "memory": "fact one"}]

    @respx.mock
    @pytest.mark.asyncio
    async def test_list_sends_user_id_query_param(self, client):
        route = respx.get("http://memory:8888/memories").mock(
            return_value=httpx.Response(200, json={"results": []})
        )
        await client.list(user_id="u-alice")
        assert route.called
        assert route.calls[0].request.url.params["user_id"] == "u-alice"

    @respx.mock
    @pytest.mark.asyncio
    async def test_list_returns_empty_on_error(self, client):
        respx.get("http://memory:8888/memories").mock(
            return_value=httpx.Response(500)
        )
        memories = await client.list(user_id="u1")
        assert memories == []

    @respx.mock
    @pytest.mark.asyncio
    async def test_delete_returns_true_on_success(self, client):
        respx.delete("http://memory:8888/memories/abc").mock(
            return_value=httpx.Response(200, json={"status": "deleted"})
        )
        ok = await client.delete(memory_id="abc")
        assert ok is True

    @respx.mock
    @pytest.mark.asyncio
    async def test_delete_returns_false_on_error(self, client):
        respx.delete("http://memory:8888/memories/abc").mock(
            return_value=httpx.Response(500)
        )
        ok = await client.delete(memory_id="abc")
        assert ok is False

    @respx.mock
    @pytest.mark.asyncio
    async def test_delete_all_sends_user_id_query_param(self, client):
        route = respx.delete("http://memory:8888/memories").mock(
            return_value=httpx.Response(200, json={"status": "deleted"})
        )
        ok = await client.delete_all(user_id="u-alice")
        assert ok is True
        assert route.called
        assert route.calls[0].request.url.params["user_id"] == "u-alice"

    @respx.mock
    @pytest.mark.asyncio
    async def test_delete_all_returns_false_on_error(self, client):
        respx.delete("http://memory:8888/memories").mock(
            return_value=httpx.Response(500)
        )
        ok = await client.delete_all(user_id="u1")
        assert ok is False

    @pytest.mark.asyncio
    async def test_aclose_closes_underlying_httpx_client(self):
        client = MemoryClient(base_url="http://memory:8888")
        assert not client._client.is_closed
        await client.aclose()
        assert client._client.is_closed

    @pytest.mark.asyncio
    async def test_aclose_is_idempotent(self):
        client = MemoryClient(base_url="http://memory:8888")
        await client.aclose()
        # Second call must not raise.
        await client.aclose()
        assert client._client.is_closed
