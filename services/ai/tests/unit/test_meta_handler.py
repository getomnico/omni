"""Unit tests for MetaToolHandler discovery/loading behavior."""

from __future__ import annotations

import pytest

from tools.connector_handler import ConnectorAction, ConnectorToolHandler
from tools.meta_handler import MetaToolHandler
from tools.searcher_client import CapabilitySearchResponse, CapabilitySearchResult
from tools.registry import ToolContext


def _make_action(
    source_id: str,
    source_type: str,
    action_name: str,
    description: str = "",
    source_name: str | None = None,
) -> ConnectorAction:
    return ConnectorAction(
        source_id=source_id,
        source_type=source_type,
        source_name=source_name or source_type,
        action_name=action_name,
        description=description,
        input_schema={"type": "object", "properties": {}},
        mode="write",
    )


def _make_handler(actions: list[ConnectorAction]) -> ConnectorToolHandler:
    handler = ConnectorToolHandler(connector_manager_url="http://unused", user_id="u1")
    handler._build_tools(actions)
    handler._initialized = True
    return handler


def _ctx() -> ToolContext:
    return ToolContext(chat_id="c1", user_id="u1")


class _FakeSearcherClient:
    def __init__(self) -> None:
        self.upserts = []
        self.searches = []

    async def upsert_capabilities(self, request):
        self.upserts.append(request)
        return type("Resp", (), {"upserted": len(request.capabilities)})()

    async def search_capabilities(self, request):
        self.searches.append(request)
        return CapabilitySearchResponse(
            results=[
                CapabilitySearchResult(
                    id="tool:gmail__send_email",
                    capability_type="tool",
                    search_text="gmail send email",
                    data={"tool_name": "gmail__send_email"},
                    score=1.0,
                )
            ]
        )


@pytest.fixture
def actions() -> list[ConnectorAction]:
    return [
        _make_action(
            "src-gmail-1",
            "gmail",
            "send_email",
            "Send an email via Gmail.",
            "Work Gmail",
        ),
        _make_action(
            "src-gmail-1",
            "gmail",
            "list_threads",
            "List recent email threads.",
            "Work Gmail",
        ),
        _make_action(
            "src-outlook-1", "outlook", "send_email", "Send an email via Outlook."
        ),
        _make_action(
            "src-drive-1", "google_drive", "create_doc", "Create a Google Doc."
        ),
        _make_action(
            "src-slack-1", "slack", "post_message", "Post a message in Slack."
        ),
    ]


@pytest.mark.asyncio
async def test_tool_search_returns_matches_without_loading(actions):
    handler = _make_handler(actions)
    loaded: set[str] = set()
    fired: list[set[str]] = []

    async def on_load(newly: set[str]) -> None:
        fired.append(newly)

    meta = MetaToolHandler(handler, loaded, on_load)
    result = await meta.execute("tool_search", {"query": "email"}, _ctx())

    assert not result.is_error
    text = result.content[0]["text"]
    assert "Found" in text
    assert "gmail__send_email" in text
    assert "outlook__send_email" in text
    assert "load_tool" in text
    assert loaded == set()
    assert fired == []


@pytest.mark.asyncio
async def test_tool_search_uses_searcher_without_loading(actions):
    handler = _make_handler(actions)
    loaded: set[str] = set()
    searcher = _FakeSearcherClient()
    meta = MetaToolHandler(handler, loaded, lambda _: None, searcher_client=searcher)
    await meta.publish_tool_capabilities()

    result = await meta.execute("tool_search", {"query": "email"}, _ctx())

    assert not result.is_error
    assert "gmail__send_email" in result.content[0]["text"]
    assert loaded == set()
    assert searcher.upserts
    assert {cap.id for cap in searcher.upserts[0].capabilities} >= {
        "tool:gmail__send_email",
        "tool:gmail__list_threads",
    }
    assert searcher.searches[0].capability_type == "tool"
    assert "tool:gmail__send_email" in searcher.searches[0].allowed_ids


@pytest.mark.asyncio
async def test_publish_tool_capabilities_skips_unchanged_refresh(actions):
    handler = _make_handler(actions)
    searcher = _FakeSearcherClient()
    meta = MetaToolHandler(handler, set(), lambda _: None, searcher_client=searcher)

    await meta.publish_tool_capabilities()
    await meta.publish_tool_capabilities()

    assert len(searcher.upserts) == 1


@pytest.mark.asyncio
async def test_tool_search_no_matches_returns_no_load(actions):
    handler = _make_handler(actions)
    loaded: set[str] = set()
    meta = MetaToolHandler(handler, loaded, lambda _: None)

    result = await meta.execute("tool_search", {"query": "xyzwhatever"}, _ctx())

    assert not result.is_error
    assert "No tools matched" in result.content[0]["text"]
    assert loaded == set()


@pytest.mark.asyncio
async def test_tool_search_respects_limit(actions):
    handler = _make_handler(actions)
    meta = MetaToolHandler(handler, set(), lambda _: None)

    result = await meta.execute(
        "tool_search", {"query": "send email", "limit": 1}, _ctx()
    )

    assert not result.is_error
    lines = [
        line for line in result.content[0]["text"].splitlines() if line.startswith("-")
    ]
    assert len(lines) == 1


@pytest.mark.asyncio
async def test_load_tool_loads_one_exact_tool(actions):
    handler = _make_handler(actions)
    loaded: set[str] = set()
    fired: list[set[str]] = []

    async def on_load(newly: set[str]) -> None:
        fired.append(newly)

    meta = MetaToolHandler(handler, loaded, on_load)
    result = await meta.execute("load_tool", {"tool_name": "gmail__send_email"}, _ctx())

    assert not result.is_error
    assert loaded == {"gmail__send_email"}
    assert fired == [{"gmail__send_email"}]
    assert "Loaded tool: gmail__send_email" in result.content[0]["text"]


@pytest.mark.asyncio
async def test_load_tool_unknown_tool_errors(actions):
    handler = _make_handler(actions)
    meta = MetaToolHandler(handler, set(), lambda _: None)

    result = await meta.execute("load_tool", {"tool_name": "missing_tool"}, _ctx())

    assert result.is_error
    assert "Unknown tool" in result.content[0]["text"]


@pytest.mark.asyncio
async def test_load_tool_set_by_source_type_loads_all_matching_tools(actions):
    handler = _make_handler(actions)
    loaded: set[str] = set()
    fired: list[set[str]] = []

    async def on_load(newly: set[str]) -> None:
        fired.append(newly)

    meta = MetaToolHandler(handler, loaded, on_load)
    result = await meta.execute("load_tool_set", {"source_type": "gmail"}, _ctx())

    assert not result.is_error
    assert loaded == {"gmail__send_email", "gmail__list_threads"}
    assert fired == [{"gmail__send_email", "gmail__list_threads"}]
    text = result.content[0]["text"]
    assert "gmail__send_email" in text
    assert "gmail__list_threads" in text


@pytest.mark.asyncio
async def test_load_tool_set_by_source_id_loads_matching_tools(actions):
    handler = _make_handler(actions)
    loaded: set[str] = set()
    meta = MetaToolHandler(handler, loaded, lambda _: None)

    result = await meta.execute("load_tool_set", {"source_id": "src-drive-1"}, _ctx())

    assert not result.is_error
    assert loaded == {"google_drive__create_doc"}


@pytest.mark.asyncio
async def test_load_tool_set_unknown_source_errors(actions):
    handler = _make_handler(actions)
    meta = MetaToolHandler(handler, set(), lambda _: None)
    result = await meta.execute("load_tool_set", {"source_type": "nonexistent"}, _ctx())
    assert result.is_error


@pytest.mark.asyncio
async def test_load_tool_set_already_loaded_skips_persist(actions):
    handler = _make_handler(actions)
    loaded: set[str] = {"gmail__send_email", "gmail__list_threads"}
    fired: list[set[str]] = []

    async def on_load(newly: set[str]) -> None:
        fired.append(newly)

    meta = MetaToolHandler(handler, loaded, on_load)
    result = await meta.execute("load_tool_set", {"source_type": "gmail"}, _ctx())

    assert not result.is_error
    assert fired == []
    assert "already loaded" in result.content[0]["text"].lower()


def test_filtered_tools_returns_only_loaded_tool_names(actions):
    handler = _make_handler(actions)

    assert handler.filtered_tools(set()) == []

    names = {t["name"] for t in handler.filtered_tools({"gmail__send_email"})}
    assert names == {"gmail__send_email"}

    multi_names = {
        t["name"]
        for t in handler.filtered_tools(
            {"gmail__send_email", "google_drive__create_doc"}
        )
    }
    assert multi_names == {"gmail__send_email", "google_drive__create_doc"}


def test_list_toolsets_groups_by_source(actions):
    handler = _make_handler(actions)
    toolsets = handler.list_toolsets()

    by_source = {ts["source_id"]: ts for ts in toolsets}
    assert by_source["src-gmail-1"]["tool_count"] == 2
    assert by_source["src-gmail-1"]["source_type"] == "gmail"
    assert by_source["src-outlook-1"]["tool_count"] == 1
    assert by_source["src-drive-1"]["source_type"] == "google_drive"
    assert "list_threads" in by_source["src-gmail-1"]["sample_tool_names"]
    assert "send_email" in by_source["src-gmail-1"]["sample_tool_names"]


def test_duplicate_source_type_actions_are_not_dropped():
    actions = [
        _make_action(
            "src-gmail-work",
            "gmail",
            "send_email",
            "Send from work Gmail.",
            "Work Gmail",
        ),
        _make_action(
            "src-gmail-personal",
            "gmail",
            "send_email",
            "Send from personal Gmail.",
            "Personal Gmail",
        ),
    ]
    handler = _make_handler(actions)

    toolsets = handler.list_toolsets()
    assert {ts["source_id"] for ts in toolsets} == {
        "src-gmail-work",
        "src-gmail-personal",
    }

    work_names = {t["name"] for t in handler.filtered_tools({"gmail__send_email"})}
    personal_names = {
        t["name"]
        for t in handler.filtered_tools(
            {"gmail__send_email__source_src_gmail_personal"}
        )
    }

    assert work_names == {"gmail__send_email"}
    assert personal_names == {"gmail__send_email__source_src_gmail_personal"}
