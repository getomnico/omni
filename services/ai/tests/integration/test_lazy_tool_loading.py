"""Integration tests for lazy connector tool loading (issue #203).

Covers the parts of the lazy-loading flow that depend on real infrastructure:
- The `chats.loaded_toolsets` column round-trips through ChatsRepository.
- Concurrent updates union (set semantics) instead of clobbering.
- The MetaToolHandler's `on_load` callback persists state through to the DB
  and survives a chat reload.
"""

from __future__ import annotations

import pytest
from ulid import ULID

import db.connection
from db import ChatsRepository, UsersRepository
from tools.connector_handler import ConnectorAction, ConnectorToolHandler
from tools.meta_handler import MetaToolHandler
from tools.registry import ToolContext

pytestmark = pytest.mark.integration


# ---------------------------------------------------------------------------
# Fixtures
# ---------------------------------------------------------------------------


@pytest.fixture
async def test_user(db_pool) -> str:
    users_repo = UsersRepository(pool=db_pool)
    user = await users_repo.create(
        email=f"{ULID()}@test.local",
        password_hash="not-a-real-hash",
        full_name="Tool Search User",
    )
    return user.id


@pytest.fixture
def _patch_db_pool(db_pool, monkeypatch):
    monkeypatch.setattr(db.connection, "_db_pool", db_pool)


def _action(
    source_id: str, source_type: str, action_name: str, description: str = ""
) -> ConnectorAction:
    return ConnectorAction(
        source_id=source_id,
        source_type=source_type,
        source_name=source_type,
        action_name=action_name,
        description=description or f"{action_name} on {source_type}",
        input_schema={"type": "object", "properties": {}},
        mode="write",
    )


def _connector_with(actions: list[ConnectorAction]) -> ConnectorToolHandler:
    handler = ConnectorToolHandler(
        connector_manager_url="http://unused",
        user_id="u1",
    )
    handler._build_tools(actions)
    handler._initialized = True
    return handler


# ---------------------------------------------------------------------------
# DB round-trip
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_chat_default_loaded_toolsets_is_empty_list(db_pool, test_user):
    repo = ChatsRepository(pool=db_pool)
    chat = await repo.create(user_id=test_user, title="t")
    assert chat.loaded_toolsets == []

    fetched = await repo.get(chat.id)
    assert fetched is not None
    assert fetched.loaded_toolsets == []


@pytest.mark.asyncio
async def test_update_loaded_toolsets_persists(db_pool, test_user):
    repo = ChatsRepository(pool=db_pool)
    chat = await repo.create(user_id=test_user, title="t")

    await repo.update_loaded_toolsets(chat.id, ["src-gmail-1", "src-drive-1"])

    fetched = await repo.get(chat.id)
    assert fetched is not None
    assert set(fetched.loaded_toolsets) == {"src-gmail-1", "src-drive-1"}


@pytest.mark.asyncio
async def test_update_loaded_toolsets_unions_with_existing(db_pool, test_user):
    """Two concurrent updates must not clobber each other."""
    repo = ChatsRepository(pool=db_pool)
    chat = await repo.create(user_id=test_user, title="t")

    await repo.update_loaded_toolsets(chat.id, ["src-gmail-1"])
    await repo.update_loaded_toolsets(chat.id, ["src-drive-1"])
    # Re-adding an existing source is idempotent (no duplicates).
    await repo.update_loaded_toolsets(chat.id, ["src-gmail-1", "src-slack-1"])

    fetched = await repo.get(chat.id)
    assert fetched is not None
    assert set(fetched.loaded_toolsets) == {
        "src-gmail-1",
        "src-drive-1",
        "src-slack-1",
    }


@pytest.mark.asyncio
async def test_update_loaded_toolsets_noop_for_empty_input(db_pool, test_user):
    repo = ChatsRepository(pool=db_pool)
    chat = await repo.create(user_id=test_user, title="t")
    await repo.update_loaded_toolsets(chat.id, ["src-gmail-1"])

    # Empty input must not wipe state.
    await repo.update_loaded_toolsets(chat.id, [])

    fetched = await repo.get(chat.id)
    assert set(fetched.loaded_toolsets) == {"src-gmail-1"}


# ---------------------------------------------------------------------------
# MetaToolHandler -> ChatsRepository persist callback
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_meta_handler_persists_through_callback(db_pool, test_user):
    """tool_search loads sources and the on_load callback writes to chats.loaded_toolsets."""
    repo = ChatsRepository(pool=db_pool)
    chat = await repo.create(user_id=test_user, title="t")

    actions = [
        _action("src-gmail-1", "gmail", "send_email", "Send email via Gmail."),
        _action("src-gmail-1", "gmail", "list_threads", "List Gmail threads."),
        _action("src-slack-1", "slack", "post_message", "Post to a Slack channel."),
    ]
    connector_handler = _connector_with(actions)

    loaded: set[str] = set()

    async def on_load(newly: set[str]) -> None:
        await repo.update_loaded_toolsets(chat.id, list(newly))

    meta = MetaToolHandler(connector_handler, loaded, on_load)
    result = await meta.execute(
        "tool_search",
        {"query": "email"},
        ToolContext(chat_id=chat.id, user_id=test_user),
    )

    assert not result.is_error
    fetched = await repo.get(chat.id)
    assert fetched is not None
    assert "src-gmail-1" in fetched.loaded_toolsets
    # Slack must not be loaded by an "email" search.
    assert "src-slack-1" not in fetched.loaded_toolsets


@pytest.mark.asyncio
async def test_loaded_toolsets_filters_subsequent_turn(db_pool, test_user):
    """After load_tool_set, filtered_tools exposes only the loaded source's tools."""
    repo = ChatsRepository(pool=db_pool)
    chat = await repo.create(user_id=test_user, title="t")

    actions = [
        _action("src-gmail-1", "gmail", "send_email"),
        _action("src-gmail-1", "gmail", "list_threads"),
        _action("src-slack-1", "slack", "post_message"),
        _action("src-drive-1", "google_drive", "create_doc"),
    ]
    connector_handler = _connector_with(actions)

    # Simulate state hydrated from DB at the start of a streaming request.
    loaded: set[str] = set(chat.loaded_toolsets)

    async def on_load(newly: set[str]) -> None:
        await repo.update_loaded_toolsets(chat.id, list(newly))

    meta = MetaToolHandler(connector_handler, loaded, on_load)

    # Turn 1: nothing loaded — filtered_tools is empty.
    assert connector_handler.filtered_tools(loaded) == []

    # LLM calls load_tool_set(source_type="gmail").
    await meta.execute(
        "load_tool_set",
        {"source_type": "gmail"},
        ToolContext(chat_id=chat.id, user_id=test_user),
    )

    # Turn 2: filtered_tools now exposes the gmail tools (and only those).
    turn2_names = {t["name"] for t in connector_handler.filtered_tools(loaded)}
    assert turn2_names == {"gmail__send_email", "gmail__list_threads"}

    # And it's persisted: a fresh repo.get() reflects the same state.
    fetched = await repo.get(chat.id)
    assert set(fetched.loaded_toolsets) == {"src-gmail-1"}


@pytest.mark.asyncio
async def test_chat_resume_restores_loaded_toolsets(db_pool, test_user):
    """A second request for the same chat starts with the previously-loaded toolsets."""
    repo = ChatsRepository(pool=db_pool)
    chat = await repo.create(user_id=test_user, title="t")

    # Simulate a prior request having loaded gmail.
    await repo.update_loaded_toolsets(chat.id, ["src-gmail-1"])

    actions = [
        _action("src-gmail-1", "gmail", "send_email"),
        _action("src-slack-1", "slack", "post_message"),
    ]
    connector_handler = _connector_with(actions)

    # New request: hydrate from DB exactly as routers/chat.py does.
    refetched = await repo.get(chat.id)
    loaded = set(refetched.loaded_toolsets)

    # Without doing any new tool_search, gmail tools are already exposed.
    names = {t["name"] for t in connector_handler.filtered_tools(loaded)}
    assert names == {"gmail__send_email"}
