"""Integration tests for the model-facing prompt/tools contract.

These tests exercise the chat SSE route, DB-backed chat/message state,
connector-manager, and searcher. The LLM is mocked only so the tests can
record the exact `system_prompt` and `tools` sent to the model.
"""

from __future__ import annotations

import json
from typing import Any

import pytest
from fastapi import FastAPI
from httpx import ASGITransport, AsyncClient
from ulid import ULID

import db.connection
from db import ChatsRepository, MessagesRepository, UsersRepository
from routers import chat_router
from state import AppState
from tests.helpers import text_response_events, tool_call_events
from tests.integration.test_background_agents import (  # noqa: F401
    searcher_container,
    searcher_image,
    searcher_url,
)
from tests.integration.test_dynamic_sources import (  # noqa: F401
    connector_manager_container,
    connector_manager_image,
    connector_manager_url,
    healthy_connector_url,
)
from tools.searcher_tool import SearcherTool

pytestmark = pytest.mark.integration

SOURCE_ID = "src-gmail-main"


class _RecordingLLM:
    PERSISTED_BLOCK_EXTRAS: tuple[str, ...] = ()
    model_name = "recording-model"
    provider_type = "test"

    def __init__(self, responses: list[tuple[str, Any]], model_id: str) -> None:
        self.responses = responses
        self.model_record_id = model_id
        self.calls: list[dict[str, Any]] = []

    async def stream_response(self, **kwargs):
        self.calls.append(kwargs)
        idx = min(len(self.calls) - 1, len(self.responses) - 1)
        kind, payload = self.responses[idx]
        if kind == "tool_call":
            for event in tool_call_events(
                payload["input"],
                tool_name=payload["name"],
                tool_id=payload.get("id", f"toolu_{idx}"),
            ):
                yield event
        else:
            for event in text_response_events(payload):
                yield event


@pytest.fixture
def _patch_db_pool(db_pool, monkeypatch):
    monkeypatch.setattr(db.connection, "_db_pool", db_pool)


@pytest.fixture
def _patch_chat_config(monkeypatch, connector_manager_url, searcher_url):
    monkeypatch.setattr("routers.chat.CONNECTOR_MANAGER_URL", connector_manager_url)
    monkeypatch.setattr("routers.chat.SANDBOX_URL", "")
    monkeypatch.setenv("SEARCHER_URL", searcher_url)


@pytest.fixture
async def chat_ids(db_pool) -> tuple[str, str, str]:
    users_repo = UsersRepository(pool=db_pool)
    user = await users_repo.create(
        email=f"{ULID()}@test.local",
        password_hash="not-a-real-hash",
        full_name="Prompt Tools User",
    )
    async with db_pool.acquire() as conn:
        provider_id = str(ULID())
        await conn.execute(
            "INSERT INTO model_providers (id, name, provider_type, config) VALUES ($1, $2, $3, $4)",
            provider_id,
            "Prompt Tools Provider",
            "anthropic",
            "{}",
        )
        model_id = str(ULID())
        await conn.execute(
            "INSERT INTO models (id, model_provider_id, model_id, display_name, is_default) VALUES ($1, $2, $3, $4, $5)",
            model_id,
            provider_id,
            "recording-model",
            "Recording Model",
            False,
        )

    chat = await ChatsRepository(pool=db_pool).create(
        user_id=user.id, model_id=model_id
    )
    return chat.id, user.id, model_id


@pytest.fixture
async def connector_state(db_pool, redis_client, healthy_connector_url, chat_ids):
    _, user_id, _ = chat_ids
    async with db_pool.acquire() as conn:
        await conn.execute("DELETE FROM sources WHERE id = $1", SOURCE_ID)
        await conn.execute(
            """INSERT INTO sources (id, name, source_type, config, is_active, is_deleted, created_by)
               VALUES ($1, $2, $3, '{}', true, false, $4)""",
            SOURCE_ID,
            "Work Gmail",
            "gmail",
            user_id,
        )

    manifest = {
        "name": "gmail_test",
        "display_name": "Gmail Test",
        "version": "1.0.0",
        "sync_modes": ["full"],
        "connector_id": "gmail_test",
        "connector_url": healthy_connector_url,
        "source_types": ["gmail"],
        "description": None,
        "actions": [
            {
                "name": "send_email",
                "description": "Send a quokka email via Gmail.",
                "input_schema": {"type": "object", "properties": {}},
                "mode": "write",
                "source_types": [],
                "admin_only": False,
            },
            {
                "name": "list_threads",
                "description": "List recent Gmail threads.",
                "input_schema": {"type": "object", "properties": {}},
                "mode": "read",
                "source_types": [],
                "admin_only": False,
            },
        ],
        "search_operators": [],
        "read_only": False,
        "extra_schema": None,
        "attributes_schema": None,
        "mcp_enabled": False,
        "prompts": [],
        "resources": [],
        "oauth": None,
    }
    await redis_client.setex("connector:manifest:gmail_test", 600, json.dumps(manifest))
    try:
        yield
    finally:
        await redis_client.delete("connector:manifest:gmail_test")
        async with db_pool.acquire() as conn:
            await conn.execute("DELETE FROM sources WHERE id = $1", SOURCE_ID)


def _build_app(llm: _RecordingLLM, model_id: str) -> FastAPI:
    app = FastAPI()
    app.state = AppState()
    app.state.models = {model_id: llm}
    app.state.default_model_id = model_id
    app.state.secondary_model_id = model_id
    app.state.searcher_tool = SearcherTool()
    app.state.content_storage = None
    app.state.redis_client = None
    app.state.memory_provider = None
    app.include_router(chat_router)
    return app


async def _add_message(
    chat_id: str, message: dict[str, Any], parent_id: str | None = None
) -> str:
    row = await MessagesRepository().create(
        chat_id=chat_id, message=message, parent_id=parent_id
    )
    return row.id


async def _stream(app: FastAPI, chat_id: str) -> str:
    async with AsyncClient(
        transport=ASGITransport(app=app), base_url="http://test"
    ) as client:
        response = await client.get(f"/chat/{chat_id}/stream", timeout=30)
        assert response.status_code == 200
        return response.text


def _tool_names(call: dict[str, Any]) -> set[str]:
    return {tool["name"] for tool in call["tools"]}


def _system_prompt(call: dict[str, Any]) -> str:
    return call["system_prompt"]


@pytest.mark.asyncio
async def test_initial_chat_turn_sends_loadable_toolsets_but_no_connector_tools(
    chat_ids,
    connector_state,
    _patch_db_pool,
    _patch_chat_config,
):
    chat_id, _, model_id = chat_ids
    await _add_message(chat_id, {"role": "user", "content": "What tools can you use?"})
    llm = _RecordingLLM(
        [("text", "I can search and load connector tools on demand.")], model_id
    )

    await _stream(_build_app(llm, model_id), chat_id)

    assert len(llm.calls) == 1
    names = _tool_names(llm.calls[0])
    assert {"tool_search", "load_tool", "load_tool_set"} <= names
    assert "search_documents" in names
    assert "gmail__send_email" not in names
    assert "gmail__list_threads" not in names

    prompt = _system_prompt(llm.calls[0])
    assert "# Loadable connector toolsets (not currently callable)" in prompt
    assert "The connector actions below are NOT in your current tool list" in prompt
    assert "gmail (source_id=" in prompt
    assert "Work Gmail" in prompt
    assert "[LOADED]" not in prompt


@pytest.mark.asyncio
async def test_load_tool_persists_for_subsequent_model_turns(
    chat_ids,
    connector_state,
    _patch_db_pool,
    _patch_chat_config,
):
    chat_id, _, model_id = chat_ids
    await _add_message(chat_id, {"role": "user", "content": "Send a quokka email"})
    llm = _RecordingLLM(
        [
            (
                "tool_call",
                {
                    "name": "tool_search",
                    "input": {"query": "send email"},
                    "id": "toolu_search_first",
                },
            ),
            (
                "tool_call",
                {
                    "name": "load_tool",
                    "input": {"tool_name": "gmail__send_email"},
                    "id": "toolu_load_one",
                },
            ),
            (
                "tool_call",
                {
                    "name": "tool_search",
                    "input": {"query": "threads"},
                    "id": "toolu_search_after_load",
                },
            ),
            ("text", "The Gmail send tool is still loaded."),
        ],
        model_id,
    )

    await _stream(_build_app(llm, model_id), chat_id)

    assert len(llm.calls) == 4
    first_turn_tools = _tool_names(llm.calls[0])
    second_turn_tools = _tool_names(llm.calls[1])
    third_turn_tools = _tool_names(llm.calls[2])
    fourth_turn_tools = _tool_names(llm.calls[3])

    assert "gmail__send_email" not in first_turn_tools
    assert "gmail__list_threads" not in first_turn_tools
    assert "gmail__send_email" not in second_turn_tools
    assert "gmail__list_threads" not in second_turn_tools
    assert "gmail__send_email" in third_turn_tools
    assert "gmail__list_threads" not in third_turn_tools
    assert "gmail__send_email" in fourth_turn_tools
    assert "gmail__list_threads" not in fourth_turn_tools
    assert {"tool_search", "load_tool", "load_tool_set"} <= second_turn_tools
    assert {"tool_search", "load_tool", "load_tool_set"} <= third_turn_tools
    assert {"tool_search", "load_tool", "load_tool_set"} <= fourth_turn_tools


@pytest.mark.asyncio
async def test_resumed_chat_reconstructs_loaded_tools_from_successful_history_only(
    chat_ids,
    connector_state,
    _patch_db_pool,
    _patch_chat_config,
):
    chat_id, _, model_id = chat_ids
    parent_id = await _add_message(
        chat_id, {"role": "user", "content": "Load Gmail send"}
    )
    parent_id = await _add_message(
        chat_id,
        {
            "role": "assistant",
            "content": [
                {
                    "type": "tool_use",
                    "id": "toolu_success",
                    "name": "load_tool",
                    "input": {"tool_name": "gmail__send_email"},
                },
                {
                    "type": "tool_use",
                    "id": "toolu_failed",
                    "name": "load_tool",
                    "input": {"tool_name": "gmail__list_threads"},
                },
            ],
        },
        parent_id=parent_id,
    )
    parent_id = await _add_message(
        chat_id,
        {
            "role": "user",
            "content": [
                {
                    "type": "tool_result",
                    "tool_use_id": "toolu_success",
                    "content": [{"type": "text", "text": "Loaded tool"}],
                    "is_error": False,
                },
                {
                    "type": "tool_result",
                    "tool_use_id": "toolu_failed",
                    "content": [{"type": "text", "text": "Unknown tool"}],
                    "is_error": True,
                },
            ],
        },
        parent_id=parent_id,
    )
    await _add_message(
        chat_id, {"role": "user", "content": "Now continue"}, parent_id=parent_id
    )
    llm = _RecordingLLM([("text", "Continuing with the loaded tool.")], model_id)

    await _stream(_build_app(llm, model_id), chat_id)

    assert len(llm.calls) == 1
    names = _tool_names(llm.calls[0])
    assert "gmail__send_email" in names
    assert "gmail__list_threads" not in names

    prompt = _system_prompt(llm.calls[0])
    assert "gmail (source_id=" in prompt
    assert "Work Gmail" in prompt
    assert "[LOADED]" in prompt
