"""Unit tests for MetaToolHandler — the tool_search and load_tool_set meta-tools.

These exercise the matching/loading logic directly against an in-memory
ConnectorToolHandler, with no LLM, no DB, and no connector-manager.
"""

from __future__ import annotations

import pytest

from tools.connector_handler import ConnectorAction, ConnectorToolHandler
from tools.meta_handler import MetaToolHandler
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
    """Construct a ConnectorToolHandler with pre-loaded actions (no HTTP)."""
    handler = ConnectorToolHandler(
        connector_manager_url="http://unused",
        user_id="u1",
    )
    handler._build_tools(actions)
    handler._initialized = True
    return handler


def _ctx() -> ToolContext:
    return ToolContext(chat_id="c1", user_id="u1")


@pytest.fixture
def actions() -> list[ConnectorAction]:
    return [
        _make_action(
            "src-gmail-1",
            "gmail",
            "send_email",
            "Send an email via Gmail.",
            source_name="Work Gmail",
        ),
        _make_action(
            "src-gmail-1",
            "gmail",
            "list_threads",
            "List recent email threads.",
            source_name="Work Gmail",
        ),
        _make_action(
            "src-outlook-1",
            "outlook",
            "send_email",
            "Send an email via Outlook.",
        ),
        _make_action(
            "src-drive-1",
            "google_drive",
            "create_doc",
            "Create a Google Doc.",
        ),
        _make_action(
            "src-slack-1",
            "slack",
            "post_message",
            "Post a message in Slack.",
        ),
    ]


# ---------------------------------------------------------------------------
# tool_search
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_tool_search_no_matches_returns_no_load(actions):
    handler = _make_handler(actions)
    loaded: set[str] = set()
    fired: list[set[str]] = []

    async def on_load(newly: set[str]) -> None:
        fired.append(newly)

    meta = MetaToolHandler(handler, loaded, on_load)
    result = await meta.execute("tool_search", {"query": "xyzwhatever"}, _ctx())

    assert not result.is_error
    text = result.content[0]["text"]
    assert "No tools matched" in text
    assert loaded == set()
    assert fired == []


@pytest.mark.asyncio
async def test_tool_search_email_loads_matching_sources(actions):
    handler = _make_handler(actions)
    loaded: set[str] = set()
    fired: list[set[str]] = []

    async def on_load(newly: set[str]) -> None:
        fired.append(newly)

    meta = MetaToolHandler(handler, loaded, on_load)
    result = await meta.execute("tool_search", {"query": "email"}, _ctx())

    assert not result.is_error
    # Both gmail and outlook source_ids should be loaded (matched on name+type).
    assert "src-gmail-1" in loaded
    assert "src-outlook-1" in loaded
    # Slack/Drive must NOT be loaded by an email-only search.
    assert "src-drive-1" not in loaded
    assert "src-slack-1" not in loaded
    # on_load fires exactly once for the union of newly-loaded sources.
    assert len(fired) == 1
    assert fired[0] == {"src-gmail-1", "src-outlook-1"}


@pytest.mark.asyncio
async def test_tool_search_skips_persist_when_already_loaded(actions):
    handler = _make_handler(actions)
    loaded: set[str] = {"src-gmail-1", "src-outlook-1"}
    fired: list[set[str]] = []

    async def on_load(newly: set[str]) -> None:
        fired.append(newly)

    meta = MetaToolHandler(handler, loaded, on_load)
    await meta.execute("tool_search", {"query": "email"}, _ctx())

    # Nothing newly loaded; persist callback must not fire.
    assert fired == []
    assert loaded == {"src-gmail-1", "src-outlook-1"}


@pytest.mark.asyncio
async def test_tool_search_respects_limit(actions):
    handler = _make_handler(actions)
    loaded: set[str] = set()

    async def on_load(_: set[str]) -> None: ...

    meta = MetaToolHandler(handler, loaded, on_load)
    # Even for a generic query that matches multiple, limit=1 caps results.
    result = await meta.execute(
        "tool_search", {"query": "send email", "limit": 1}, _ctx()
    )

    assert not result.is_error
    # Only one source loaded since limit=1.
    assert len(loaded) == 1


@pytest.mark.asyncio
async def test_tool_search_missing_query_errors(actions):
    handler = _make_handler(actions)
    meta = MetaToolHandler(handler, set(), lambda _: None)
    result = await meta.execute("tool_search", {}, _ctx())
    assert result.is_error
    assert "query" in result.content[0]["text"].lower()


# ---------------------------------------------------------------------------
# load_tool_set
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_load_tool_set_by_source_type(actions):
    handler = _make_handler(actions)
    loaded: set[str] = set()
    fired: list[set[str]] = []

    async def on_load(newly: set[str]) -> None:
        fired.append(newly)

    meta = MetaToolHandler(handler, loaded, on_load)
    result = await meta.execute("load_tool_set", {"source_type": "gmail"}, _ctx())

    assert not result.is_error
    assert loaded == {"src-gmail-1"}
    assert fired == [{"src-gmail-1"}]
    text = result.content[0]["text"]
    assert "gmail__send_email" in text
    assert "gmail__list_threads" in text


@pytest.mark.asyncio
async def test_load_tool_set_by_source_id(actions):
    handler = _make_handler(actions)
    loaded: set[str] = set()

    async def on_load(_: set[str]) -> None: ...

    meta = MetaToolHandler(handler, loaded, on_load)
    result = await meta.execute("load_tool_set", {"source_id": "src-drive-1"}, _ctx())

    assert not result.is_error
    assert loaded == {"src-drive-1"}


@pytest.mark.asyncio
async def test_load_tool_set_unknown_source_errors(actions):
    handler = _make_handler(actions)
    meta = MetaToolHandler(handler, set(), lambda _: None)
    result = await meta.execute("load_tool_set", {"source_type": "nonexistent"}, _ctx())
    assert result.is_error


@pytest.mark.asyncio
async def test_load_tool_set_already_loaded_skips_persist(actions):
    handler = _make_handler(actions)
    loaded: set[str] = {"src-gmail-1"}
    fired: list[set[str]] = []

    async def on_load(newly: set[str]) -> None:
        fired.append(newly)

    meta = MetaToolHandler(handler, loaded, on_load)
    result = await meta.execute("load_tool_set", {"source_type": "gmail"}, _ctx())

    assert not result.is_error
    assert fired == []  # already loaded, no persist
    assert "already loaded" in result.content[0]["text"].lower()


@pytest.mark.asyncio
async def test_load_tool_set_missing_args_errors(actions):
    handler = _make_handler(actions)
    meta = MetaToolHandler(handler, set(), lambda _: None)
    result = await meta.execute("load_tool_set", {}, _ctx())
    assert result.is_error


# ---------------------------------------------------------------------------
# Connector handler integration
# ---------------------------------------------------------------------------


def test_filtered_tools_returns_only_loaded_sources(actions):
    handler = _make_handler(actions)

    # Empty load set => no connector tools exposed.
    assert handler.filtered_tools(set()) == []

    # Load just gmail.
    gmail_tools = handler.filtered_tools({"src-gmail-1"})
    names = {t["name"] for t in gmail_tools}
    assert names == {"gmail__send_email", "gmail__list_threads"}

    # Load gmail + drive.
    multi_tools = handler.filtered_tools({"src-gmail-1", "src-drive-1"})
    multi_names = {t["name"] for t in multi_tools}
    assert "google_drive__create_doc" in multi_names
    assert "gmail__send_email" in multi_names
    assert "slack__post_message" not in multi_names


def test_list_toolsets_groups_by_source(actions):
    handler = _make_handler(actions)
    toolsets = handler.list_toolsets()

    by_source = {ts["source_id"]: ts for ts in toolsets}
    assert by_source["src-gmail-1"]["tool_count"] == 2
    assert by_source["src-gmail-1"]["source_type"] == "gmail"
    assert by_source["src-outlook-1"]["tool_count"] == 1
    assert by_source["src-drive-1"]["source_type"] == "google_drive"
    # Sample names are sorted action_names, capped at 3.
    assert "list_threads" in by_source["src-gmail-1"]["sample_tool_names"]
    assert "send_email" in by_source["src-gmail-1"]["sample_tool_names"]


def test_duplicate_source_type_actions_are_not_dropped():
    actions = [
        _make_action(
            "src-gmail-work",
            "gmail",
            "send_email",
            "Send from work Gmail.",
            source_name="Work Gmail",
        ),
        _make_action(
            "src-gmail-personal",
            "gmail",
            "send_email",
            "Send from personal Gmail.",
            source_name="Personal Gmail",
        ),
    ]
    handler = _make_handler(actions)

    toolsets = handler.list_toolsets()
    assert {ts["source_id"] for ts in toolsets} == {
        "src-gmail-work",
        "src-gmail-personal",
    }

    work_names = {t["name"] for t in handler.filtered_tools({"src-gmail-work"})}
    personal_names = {t["name"] for t in handler.filtered_tools({"src-gmail-personal"})}

    assert work_names == {"gmail__send_email"}
    assert personal_names == {"gmail__send_email__source_src_gmail_personal"}
