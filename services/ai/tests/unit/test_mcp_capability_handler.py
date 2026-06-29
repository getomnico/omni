from __future__ import annotations

import json

import pytest
import respx
from httpx import Response

from db.models import Source
from tools.mcp_capability_handler import McpCapabilityHandler
from tools.registry import ToolContext
from tools.searcher_client import CapabilitySearchResponse, CapabilitySearchResult


class _FakeSearcherClient:
    def __init__(self) -> None:
        self.upserts = []
        self.searches = []
        self.results: list[CapabilitySearchResult] = []

    async def upsert_capabilities(self, request):
        self.upserts.append(request)
        return type("Resp", (), {"upserted": len(request.capabilities)})()

    async def search_capabilities(self, request):
        self.searches.append(request)
        results = [
            result
            for result in self.results
            if result.capability_type == request.capability_type
            and (request.allowed_ids is None or result.id in request.allowed_ids)
            and (
                request.allowed_source_ids is None
                or result.source_id in request.allowed_source_ids
            )
        ]
        return CapabilitySearchResponse(results=results[: request.limit])


def _source(source_id: str, source_type: str = "docs", *, active: bool = True) -> Source:
    return Source(
        id=source_id,
        name=f"{source_type} source",
        source_type=source_type,
        is_active=active,
        is_deleted=False,
    )


def _ctx() -> ToolContext:
    return ToolContext(chat_id="chat-1", user_id="user-1")


def _manifest() -> dict:
    return {
        "mcp_enabled": True,
        "resources": [
            {
                "uri_template": "docs://guide",
                "name": "Guide",
                "description": "Reference guide",
                "mime_type": "text/plain",
            },
            {
                "uri_template": "docs://tickets/{ticket_id}",
                "name": "Ticket",
                "description": "Ticket details",
                "mime_type": "text/plain",
            },
        ],
        "prompts": [
            {
                "name": "debug_error",
                "description": "Debug an error",
                "arguments": [
                    {"name": "error", "description": "Error text", "required": True}
                ],
            }
        ],
    }


@pytest.fixture(autouse=True)
def _clear_publish_cache():
    McpCapabilityHandler._published_capability_keys.clear()


@pytest.mark.asyncio
@respx.mock
async def test_publishes_resource_and_prompt_capabilities() -> None:
    searcher = _FakeSearcherClient()
    respx.get("http://cm.test/connectors").mock(
        return_value=Response(
            200,
            json=[
                {
                    "source_type": "docs",
                    "healthy": True,
                    "manifest": _manifest(),
                }
            ],
        )
    )
    handler = McpCapabilityHandler(
        "http://cm.test", searcher_client=searcher, prefetched_sources=[_source("src-1")]
    )

    await handler.publish_capabilities()

    assert len(searcher.upserts) == 1
    capabilities = searcher.upserts[0].capabilities
    assert {cap.capability_type for cap in capabilities} == {"resource", "prompt"}
    resource_caps = [cap for cap in capabilities if cap.capability_type == "resource"]
    prompt_caps = [cap for cap in capabilities if cap.capability_type == "prompt"]
    assert len(resource_caps) == 2
    assert len(prompt_caps) == 1
    assert all(cap.source_id == "src-1" for cap in capabilities)
    guide = next(cap for cap in resource_caps if cap.data["name"] == "Guide")
    assert guide.id.startswith("resource:src-1:")
    assert guide.data["uri_template"] == "docs://guide"
    prompt = prompt_caps[0]
    assert prompt.id == "prompt:src-1:debug_error"
    assert prompt.data["arguments"][0]["required"] is True


@pytest.mark.asyncio
@respx.mock
async def test_search_uses_allowed_ids_and_source_ids() -> None:
    searcher = _FakeSearcherClient()
    respx.get("http://cm.test/connectors").mock(
        return_value=Response(200, json=[{"source_type": "docs", "healthy": True, "manifest": _manifest()}])
    )
    handler = McpCapabilityHandler(
        "http://cm.test", searcher_client=searcher, prefetched_sources=[_source("src-1")]
    )
    await handler.refresh()
    resource_id = next(iter(handler._resources))
    prompt_id = next(iter(handler._prompts))
    searcher.results = [
        CapabilitySearchResult(
            id=resource_id,
            capability_type="resource",
            name="Guide",
            description="Reference guide",
            search_text="guide",
            data={},
            score=1.0,
            source_id="src-1",
            source_type="docs",
        ),
        CapabilitySearchResult(
            id=prompt_id,
            capability_type="prompt",
            name="debug_error",
            description="Debug an error",
            search_text="debug",
            data={},
            score=1.0,
            source_id="src-1",
            source_type="docs",
        ),
    ]

    resource_result = await handler.execute("resource_search", {"query": "guide"}, _ctx())
    prompt_result = await handler.execute("prompt_search", {"query": "debug"}, _ctx())

    assert resource_id in resource_result.content[0]["text"]
    assert prompt_id in prompt_result.content[0]["text"]
    assert searcher.searches[0].capability_type == "resource"
    assert resource_id in searcher.searches[0].allowed_ids
    assert searcher.searches[0].allowed_source_ids == ["src-1"]
    assert searcher.searches[1].capability_type == "prompt"
    assert prompt_id in searcher.searches[1].allowed_ids


@pytest.mark.asyncio
@respx.mock
async def test_load_resource_requires_uri_for_templates() -> None:
    respx.get("http://cm.test/connectors").mock(
        return_value=Response(200, json=[{"source_type": "docs", "healthy": True, "manifest": _manifest()}])
    )
    handler = McpCapabilityHandler("http://cm.test", prefetched_sources=[_source("src-1")])
    await handler.refresh()
    template_id = next(
        record.id for record in handler._resources.values() if record.requires_uri
    )

    result = await handler.execute("load_resource", {"resource_id": template_id}, _ctx())

    assert result.is_error
    assert "provide a concrete uri" in result.content[0]["text"]


@pytest.mark.asyncio
@respx.mock
async def test_load_resource_applies_line_range() -> None:
    respx.get("http://cm.test/connectors").mock(
        return_value=Response(200, json=[{"source_type": "docs", "healthy": True, "manifest": _manifest()}])
    )
    respx.post("http://cm.test/resource").mock(
        return_value=Response(
            200,
            json={"contents": [{"uri": "docs://guide", "text": "one\ntwo\nthree\nfour", "mime_type": "text/plain"}]},
        )
    )
    handler = McpCapabilityHandler("http://cm.test", prefetched_sources=[_source("src-1")])
    await handler.refresh()
    guide_id = next(
        record.id for record in handler._resources.values() if record.name == "Guide"
    )

    result = await handler.execute(
        "load_resource",
        {"resource_id": guide_id, "start_line": 2, "end_line": 3},
        _ctx(),
    )

    assert not result.is_error
    text = result.content[0]["text"]
    assert "Returned lines 2-3 of 4" in text
    assert "two\nthree" in text
    assert "one\n" not in text


@pytest.mark.asyncio
@respx.mock
async def test_load_resource_large_content_returns_preview_and_reload_instruction() -> None:
    respx.get("http://cm.test/connectors").mock(
        return_value=Response(200, json=[{"source_type": "docs", "healthy": True, "manifest": _manifest()}])
    )
    large_text = "\n".join(f"line {idx} " + "x" * 200 for idx in range(1, 220))
    respx.post("http://cm.test/resource").mock(
        return_value=Response(200, json={"contents": [{"uri": "docs://guide", "text": large_text}]})
    )
    handler = McpCapabilityHandler("http://cm.test", prefetched_sources=[_source("src-1")])
    await handler.refresh()
    guide_id = next(
        record.id for record in handler._resources.values() if record.name == "Guide"
    )

    result = await handler.execute("load_resource", {"resource_id": guide_id}, _ctx())

    assert not result.is_error
    text = result.content[0]["text"]
    assert "too large to include inline" in text
    assert "Preview lines 1-" in text
    assert "start_line=<line>, end_line=<line>" in text


@pytest.mark.asyncio
@respx.mock
async def test_load_prompt_validates_required_arguments() -> None:
    respx.get("http://cm.test/connectors").mock(
        return_value=Response(200, json=[{"source_type": "docs", "healthy": True, "manifest": _manifest()}])
    )
    handler = McpCapabilityHandler("http://cm.test", prefetched_sources=[_source("src-1")])
    await handler.refresh()
    prompt_id = next(iter(handler._prompts))

    result = await handler.execute("load_prompt", {"prompt_id": prompt_id}, _ctx())

    assert result.is_error
    assert "Missing required prompt argument" in result.content[0]["text"]


@pytest.mark.asyncio
@respx.mock
async def test_load_prompt_returns_structured_template_tool_result() -> None:
    respx.get("http://cm.test/connectors").mock(
        return_value=Response(200, json=[{"source_type": "docs", "healthy": True, "manifest": _manifest()}])
    )
    prompt_route = respx.post("http://cm.test/prompt").mock(
        return_value=Response(
            200,
            json={
                "description": "Debug prompt",
                "messages": [
                    {"role": "user", "content": {"type": "text", "text": "I'm seeing this error:"}},
                    {"role": "user", "content": {"type": "text", "text": "boom"}},
                    {"role": "assistant", "content": {"type": "text", "text": "I'll help debug."}},
                ],
            },
        )
    )
    handler = McpCapabilityHandler("http://cm.test", prefetched_sources=[_source("src-1")])
    await handler.refresh()
    prompt_id = next(iter(handler._prompts))

    result = await handler.execute(
        "load_prompt",
        {"prompt_id": prompt_id, "arguments": {"error": "boom"}},
        _ctx(),
    )

    assert not result.is_error
    assert prompt_route.calls[0].request.content
    body = json.loads(prompt_route.calls[0].request.content)
    assert body == {"source_id": "src-1", "name": "debug_error", "arguments": {"error": "boom"}}
    text = result.content[0]["text"]
    assert "not actual user/assistant chat history" in text
    assert "```json" in text
    data = json.loads(text.split("```json\n", 1)[1].rsplit("\n```", 1)[0])
    assert data["prompt_id"] == prompt_id
    assert [message["role"] for message in data["messages"]] == ["user", "user", "assistant"]
    assert data["messages"][0]["content"]["text"] == "I'm seeing this error:"


@pytest.mark.asyncio
@respx.mock
async def test_source_filter_limits_records_to_readable_sources() -> None:
    respx.get("http://cm.test/connectors").mock(
        return_value=Response(200, json=[{"source_type": "docs", "healthy": True, "manifest": _manifest()}])
    )
    handler = McpCapabilityHandler(
        "http://cm.test",
        prefetched_sources=[_source("src-read"), _source("src-write")],
        source_filter={"src-read": ["read"], "src-write": ["write"]},
    )

    await handler.refresh()

    assert handler.has_capabilities()
    assert {record.source_id for record in handler._resources.values()} == {"src-read"}
    assert {record.source_id for record in handler._prompts.values()} == {"src-read"}
