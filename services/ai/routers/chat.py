"""Chat streaming API routes.

Thin router extracted from the historical monolith.  Orchestrates request
setup (chat lookup, registry build, compaction, provider resolution) and
delegates the heavy streaming work to ``services/ai/streaming/`` modules.
"""

from __future__ import annotations

import asyncio
import json
import logging
import pathlib
from dataclasses import dataclass
from typing import Any, cast

import httpx
from anthropic.types import (
    ContentBlockParam,
    MessageParam,
    TextBlockParam,
    ToolResultBlockParam,
    ToolUseBlockParam,
)
from fastapi import APIRouter, HTTPException, Path, Query, Request
from fastapi.responses import Response, StreamingResponse

from agents.executor import _build_source_filter
from agents.models import Agent
from agents.repository import AgentRepository, AgentRunRepository
from attachments import expand_uploads
from config import (
    AGENT_MAX_ITERATIONS,
    CONNECTOR_MANAGER_URL,
    DEFAULT_MAX_TOKENS,
    DEFAULT_TEMPERATURE,
    DEFAULT_TOP_P,
    SANDBOX_URL,
)
from db import ChatsRepository, CompactionsRepository, MessagesRepository
from db.documents import DocumentsRepository
from db.configuration import ConfigurationRepository
from db.models import Chat, Source, UserConfiguration
from db.tool_approvals import (
    ToolApproval,
    ToolApprovalStatus,
    ToolApprovalType,
    ToolApprovalsRepository,
)
from db.uploads import UploadsRepository
from db.usage import UsageRepository
from db.users import UsersRepository
from memory import (
    MemoryMode,
    agent_key,
    resolve_memory_mode,
    user_key,
)
from prompts import build_agent_chat_system_prompt, build_chat_system_prompt
from providers import LLMProvider
from services.compaction import ConversationCompactor
from services.title_generation import generate_title_for_conversation
from services.usage import UsageContext, UsagePurpose, UsageTracker, track_usage
from state import AppState
from streaming.generate import (
    active_path_tool_call_ids,
    drop_empty_assistant_messages,
    message_content_blocks,
    repair_interrupted_tool_calls,
    stream_generator,
    tool_use_blocks,
)
from streaming.persist import (
    EndOfStreamReason,
    end_of_stream,
    persist_and_transform,
)
from streaming.run import (
    SSE_HEADERS,
    _CANCEL_TTL,
    _RUN_LOCK_TTL,
    _run_tasks_by_chat,
    cancel_key,
    clear_producer_task,
    consume_run,
    run_lock_key,
    run_producer,
    set_producer_task,
    stream_key,
)
from tools import (
    ConnectorToolHandler,
    DocumentToolHandler,
    PeopleSearchHandler,
    SearchToolHandler,
    WebToolHandler,
    ToolContext,
    ToolHandler,
    ToolRegistry,
)
from tools.connector_handler import (
    SearchOperator,
    ToolsetSummary,
    sources_from_sync_overview_response,
)
from tools.meta_handler import MetaToolHandler, OnLoad
from tools.mcp_capability_handler import McpCapabilityHandler
from tools.omni_tool_result import OAuthRequiredPayload
from tools.sandbox_handler import SandboxToolHandler
from tools.search_handler import fetch_operator_values
from tools.skill_handler import SkillHandler
from tools.turn_builder import build_turn_tools

router = APIRouter(tags=["chat"])
logger = logging.getLogger(__name__)


# ---------------------------------------------------------------------------
# Provider resolution helpers
# ---------------------------------------------------------------------------


def _resolve_provider(state: AppState, model_id: str | None) -> LLMProvider:
    models = state.models
    if not models:
        raise HTTPException(status_code=503, detail="No models configured")

    if model_id and model_id in models:
        return models[model_id]
    if state.default_model_id and state.default_model_id in models:
        return models[state.default_model_id]
    return next(iter(models.values()))


def _resolve_llm_provider(state: AppState, chat: Chat) -> LLMProvider:
    return _resolve_provider(state, chat.model_id)


def _resolve_secondary_provider(state: AppState) -> LLMProvider:
    return _resolve_provider(state, state.secondary_model_id or state.default_model_id)


# ---------------------------------------------------------------------------
# Registry building helpers
# ---------------------------------------------------------------------------


@dataclass
class RegistryResult:
    registry: ToolRegistry
    always_on_handlers: list[ToolHandler]
    connector_handler: ConnectorToolHandler | None
    toolsets: list[ToolsetSummary]
    sources: list[Source]
    search_operators: list[SearchOperator]


def _loaded_tools_from_history(
    messages: list[MessageParam], connector_handler: ConnectorToolHandler
) -> set[str]:
    tool_calls: dict[str, ToolUseBlockParam] = {}
    loaded: set[str] = set()

    for message in messages:
        for block in message_content_blocks(message):
            match block["type"]:
                case "tool_use":
                    tool_use = cast(ToolUseBlockParam, block)
                    tool_calls[tool_use["id"]] = tool_use
                case "tool_result":
                    tool_result = cast(ToolResultBlockParam, block)
                    if tool_result.get("is_error", False):
                        continue
                    call = tool_calls.get(tool_result["tool_use_id"])
                    if call is None:
                        continue
                    loaded.update(
                        _loaded_tools_from_meta_call(
                            call["name"], call["input"], connector_handler
                        )
                    )
                case _:
                    continue

    return loaded


def _loaded_tools_from_meta_call(
    tool_name: str,
    tool_input: dict[str, object],
    connector_handler: ConnectorToolHandler,
) -> set[str]:
    if tool_name == "load_tool":
        requested = tool_input.get("tool_name")
        if isinstance(requested, str) and requested in connector_handler.actions:
            return {requested}
    if tool_name == "load_tool_set":
        source_id = tool_input.get("source_id")
        if isinstance(source_id, str) and source_id:
            return {
                name
                for name, action in connector_handler.actions.items()
                if action.source_id == source_id
            }
        source_type = tool_input.get("source_type")
        if isinstance(source_type, str) and source_type:
            return {
                name
                for name, action in connector_handler.actions.items()
                if action.source_type == source_type
            }
    return set()


def _loaded_source_ids(
    loaded_tool_names: set[str], connector_handler: ConnectorToolHandler | None
) -> set[str]:
    if connector_handler is None:
        return set()
    return {
        action.source_id
        for tool_name, action in connector_handler.actions.items()
        if tool_name in loaded_tool_names
    }


async def _fetch_sources_from_connector_manager() -> list[Source] | None:
    try:
        async with httpx.AsyncClient(timeout=10.0) as client:
            resp = await client.get(f"{CONNECTOR_MANAGER_URL.rstrip('/')}/sources")
            resp.raise_for_status()
            return sources_from_sync_overview_response(resp.json())
    except Exception as e:
        logger.warning(f"Failed to fetch sources from connector manager: {e}")
        return None


async def _noop_on_load(_: set[str]) -> None:
    return None


async def _build_registry(
    request: Request,
    chat: Chat,
    is_admin: bool,
    loaded_toolsets: set[str],
    on_load: OnLoad | None = None,
) -> RegistryResult:
    registry = ToolRegistry()
    always_on_handlers: list[ToolHandler] = []

    sources = await _fetch_sources_from_connector_manager() or []

    connector_handler: ConnectorToolHandler | None = None
    toolsets: list[ToolsetSummary] = []
    search_operators: list[SearchOperator] = []

    connector_handler = ConnectorToolHandler(
        connector_manager_url=CONNECTOR_MANAGER_URL,
        user_id=chat.user_id,
        redis_client=request.app.state.redis_client,
        prefetched_sources=sources,
        documents_repo=DocumentsRepository(),
        sandbox_url=SANDBOX_URL,
        is_admin=is_admin,
    )
    await connector_handler._ensure_initialized()
    registry.register(connector_handler)

    if connector_handler.actions:
        toolsets = connector_handler.list_toolsets()

    if connector_handler.search_operators:
        search_operators = connector_handler.search_operators

    if connector_handler is not None and toolsets:
        meta_handler = MetaToolHandler(
            connector_handler=connector_handler,
            loaded=loaded_toolsets,
            on_load=on_load or _noop_on_load,
            searcher_client=request.app.state.searcher_tool.client,
        )
        await meta_handler.publish_tool_capabilities()
        registry.register(meta_handler)
        always_on_handlers.append(meta_handler)

    mcp_handler = McpCapabilityHandler(
        connector_manager_url=CONNECTOR_MANAGER_URL,
        searcher_client=request.app.state.searcher_tool.client,
        prefetched_sources=sources,
    )
    await mcp_handler.refresh()
    if mcp_handler.has_capabilities():
        await mcp_handler.publish_capabilities()
        registry.register(mcp_handler)
        always_on_handlers.append(mcp_handler)

    active_sources = [s for s in sources if s.is_active and not s.is_deleted]
    connected_source_types = list({s.source_type for s in active_sources})
    operator_values: dict[str, list[str]] = {}
    if search_operators:
        operator_values = await fetch_operator_values(
            request.app.state.searcher_tool.client,
            search_operators,
            redis_client=request.app.state.redis_client,
        )

    search_handler = SearchToolHandler(
        searcher_tool=request.app.state.searcher_tool,
        search_operators=search_operators,
        connected_source_types=connected_source_types,
        operator_values=operator_values,
    )
    registry.register(search_handler)
    always_on_handlers.append(search_handler)

    web_search_provider = getattr(request.app.state, "web_search_provider", None)
    if web_search_provider is not None:
        web_handler = WebToolHandler(
            search_provider=web_search_provider,
            fetch_provider=getattr(request.app.state, "web_fetch_provider", None),
        )
        registry.register(web_handler)
        always_on_handlers.append(web_handler)

    people_handler = PeopleSearchHandler(searcher_tool=request.app.state.searcher_tool)
    registry.register(people_handler)
    always_on_handlers.append(people_handler)

    content_storage = getattr(request.app.state, "content_storage", None)
    document_handler = DocumentToolHandler(
        content_storage=content_storage,
        documents_repo=DocumentsRepository(),
        sandbox_url=SANDBOX_URL,
        connector_manager_url=CONNECTOR_MANAGER_URL,
    )
    registry.register(document_handler)
    always_on_handlers.append(document_handler)

    if SANDBOX_URL:
        sandbox_handler = SandboxToolHandler(sandbox_url=SANDBOX_URL)
        registry.register(sandbox_handler)
        always_on_handlers.append(sandbox_handler)

    skills_dir = pathlib.Path(__file__).resolve().parent.parent / "skills"
    skill_handler = SkillHandler(
        skills_dir=skills_dir,
        searcher_client=request.app.state.searcher_tool.client,
        connector_manager_url=CONNECTOR_MANAGER_URL,
    )
    await skill_handler.refresh_connector_skills()
    if skill_handler.has_skills():
        await skill_handler.publish_skill_capabilities()
        registry.register(skill_handler)
        always_on_handlers.append(skill_handler)

    return RegistryResult(
        registry=registry,
        always_on_handlers=always_on_handlers,
        connector_handler=connector_handler,
        toolsets=toolsets,
        sources=sources,
        search_operators=search_operators,
    )


async def _build_agent_chat_registry(
    request: Request, agent: Agent, is_admin: bool
) -> RegistryResult:
    registry = ToolRegistry()
    always_on_handlers: list[ToolHandler] = []

    sources = await _fetch_sources_from_connector_manager() or []

    source_filter = _build_source_filter(agent) if agent.agent_type == "user" else None

    search_operators: list[SearchOperator] = []
    connector_handler = ConnectorToolHandler(
        connector_manager_url=CONNECTOR_MANAGER_URL,
        user_id=agent.user_id if agent.agent_type == "user" else "",
        redis_client=request.app.state.redis_client,
        prefetched_sources=sources,
        source_filter=source_filter,
        documents_repo=DocumentsRepository(),
        is_admin=is_admin,
    )
    await connector_handler._ensure_initialized()
    if connector_handler.search_operators:
        search_operators = connector_handler.search_operators

    mcp_handler = McpCapabilityHandler(
        connector_manager_url=CONNECTOR_MANAGER_URL,
        searcher_client=request.app.state.searcher_tool.client,
        prefetched_sources=sources,
        source_filter=source_filter,
    )
    await mcp_handler.refresh()
    if mcp_handler.has_capabilities():
        await mcp_handler.publish_capabilities()
        registry.register(mcp_handler)
        always_on_handlers.append(mcp_handler)

    active_sources = [s for s in sources if s.is_active and not s.is_deleted]
    connected_source_types = list({s.source_type for s in active_sources})
    operator_values: dict[str, list[str]] = {}
    if search_operators:
        operator_values = await fetch_operator_values(
            request.app.state.searcher_tool.client,
            search_operators,
            redis_client=request.app.state.redis_client,
        )

    search_handler = SearchToolHandler(
        searcher_tool=request.app.state.searcher_tool,
        search_operators=search_operators,
        connected_source_types=connected_source_types,
        operator_values=operator_values,
    )
    registry.register(search_handler)
    always_on_handlers.append(search_handler)

    web_search_provider = getattr(request.app.state, "web_search_provider", None)
    if web_search_provider is not None:
        web_handler = WebToolHandler(
            search_provider=web_search_provider,
            fetch_provider=getattr(request.app.state, "web_fetch_provider", None),
        )
        registry.register(web_handler)
        always_on_handlers.append(web_handler)

    people_handler = PeopleSearchHandler(searcher_tool=request.app.state.searcher_tool)
    registry.register(people_handler)
    always_on_handlers.append(people_handler)

    content_storage = getattr(request.app.state, "content_storage", None)
    document_handler = DocumentToolHandler(
        content_storage=content_storage,
        documents_repo=DocumentsRepository(),
        sandbox_url=SANDBOX_URL,
        connector_manager_url=CONNECTOR_MANAGER_URL,
    )
    registry.register(document_handler)
    always_on_handlers.append(document_handler)

    if SANDBOX_URL:
        sandbox_handler = SandboxToolHandler(sandbox_url=SANDBOX_URL)
        registry.register(sandbox_handler)
        always_on_handlers.append(sandbox_handler)

    skills_dir = pathlib.Path(__file__).resolve().parent.parent / "skills"
    skill_handler = SkillHandler(
        skills_dir=skills_dir,
        searcher_client=request.app.state.searcher_tool.client,
        connector_manager_url=CONNECTOR_MANAGER_URL,
    )
    await skill_handler.refresh_connector_skills()
    if skill_handler.has_skills():
        await skill_handler.publish_skill_capabilities()
        registry.register(skill_handler)
        always_on_handlers.append(skill_handler)

    return RegistryResult(
        registry=registry,
        always_on_handlers=always_on_handlers,
        connector_handler=None,
        toolsets=[],
        sources=sources,
        search_operators=search_operators,
    )


# ---------------------------------------------------------------------------
# Title-generation helper
# ---------------------------------------------------------------------------


def _extract_text_for_title(
    content: str | list[ContentBlockParam] | None,
) -> str | None:
    if isinstance(content, str):
        text = content.strip()
        return text if text else None

    if not isinstance(content, list):
        return None

    parts: list[str] = []
    for block in content:
        if not isinstance(block, dict):
            continue
        if block.get("type") != "text":
            continue
        text = block.get("text")
        if isinstance(text, str) and text.strip():
            parts.append(text.strip())

    if not parts:
        return None
    return "\n".join(parts)


# ---------------------------------------------------------------------------
# Route: stream status
# ---------------------------------------------------------------------------


@router.get("/chat/{chat_id}/stream/status")
async def stream_status(
    request: Request, chat_id: str = Path(..., description="Chat thread ID")
):
    redis_client = getattr(request.app.state, "redis_client", None)
    messages_repo = MessagesRepository()
    approvals_repo = ToolApprovalsRepository()
    active_tool_call_ids_set = await active_path_tool_call_ids(messages_repo, chat_id)
    pending_approval = bool(
        await approvals_repo.list_for_chat(
            chat_id=chat_id,
            approval_type=ToolApprovalType.APPROVAL,
            statuses={ToolApprovalStatus.PENDING},
            active_tool_call_ids=active_tool_call_ids_set,
        )
    )
    pending_oauth = bool(
        await approvals_repo.list_for_chat(
            chat_id=chat_id,
            approval_type=ToolApprovalType.OAUTH,
            statuses={ToolApprovalStatus.PENDING},
            active_tool_call_ids=active_tool_call_ids_set,
        )
    )
    if redis_client is None:
        raise HTTPException(status_code=500, detail="Redis client is not initialized")

    return {
        "running": bool(await redis_client.exists(run_lock_key(chat_id))),
        "resumable": bool(await redis_client.exists(stream_key(chat_id))),
        "pending_approval": pending_approval,
        "pending_oauth": pending_oauth,
    }


# ---------------------------------------------------------------------------
# Route: stream chat (main handler)
# ---------------------------------------------------------------------------


# ---------------------------------------------------------------------------
# Stream chat handler
# ---------------------------------------------------------------------------

class StreamChatHandler:
    def __init__(self, request: Request, chat_id: str, auto_start: bool) -> None:
        self.request = request
        self.chat_id = chat_id
        self.auto_start = auto_start

    async def handle(self) -> StreamingResponse:
        """Stream AI response for a chat thread using Server-Sent Events"""
        request = self.request
        chat_id = self.chat_id
        auto_start = self.auto_start

        if not request.app.state.searcher_tool:
            raise HTTPException(status_code=500, detail="Searcher tool not initialized")

        chats_repo = ChatsRepository()
        chat = await chats_repo.get(chat_id)
        if not chat:
            raise HTTPException(status_code=404, detail="Chat thread not found")

        llm_provider = _resolve_llm_provider(request.app.state, chat)
        redis_client = request.app.state.redis_client

        # Reconnect/resume fast path
        last_event_id = request.headers.get("last-event-id") or request.query_params.get(
            "last_event_id"
        )
        if redis_client is not None:
            run_active = await redis_client.exists(run_lock_key(chat_id))
            if run_active:
                return StreamingResponse(
                    consume_run(redis_client, chat_id, last_event_id or "0"),
                    media_type="text/event-stream",
                    headers=SSE_HEADERS,
                )

            if last_event_id is not None:
                if await redis_client.exists(stream_key(chat_id)):
                    return StreamingResponse(
                        consume_run(redis_client, chat_id, last_event_id),
                        media_type="text/event-stream",
                        headers=SSE_HEADERS,
                    )

                async def _not_resumable_response():
                    yield "event: not_resumable\ndata: \n\n"

                return StreamingResponse(
                    _not_resumable_response(),
                    media_type="text/event-stream",
                    headers=SSE_HEADERS,
                )

        messages_repo = MessagesRepository()
        approvals_repo = ToolApprovalsRepository()
        chat_messages = await messages_repo.get_active_path(chat_id)

        # Shared state scoped to this request
        memory_provider = None
        effective_mode = MemoryMode.OFF
        memories: list[str] = []
        memory_write_key: str | None = None
        pending: list[ToolApproval] = []
        pending_oauth: list[ToolApproval] = []

        if chat.agent_id:
            # ---- Agent chat setup ----
            agent_repo = AgentRepository()
            agent = await agent_repo.get_agent(chat.agent_id)
            if not agent:
                raise HTTPException(status_code=404, detail="Agent not found")

            users_repo = UsersRepository()
            chat_user = await users_repo.find_by_id(chat.user_id)
            if not chat_user:
                raise HTTPException(status_code=404, detail="Chat user not found")

            if agent.agent_type == "org":
                if chat_user.role != "admin":
                    raise HTTPException(
                        status_code=403,
                        detail="Admin access required for org agent chats",
                    )
            elif agent.user_id != chat.user_id:
                raise HTTPException(
                    status_code=403,
                    detail="Only the agent owner can chat with this agent",
                )

            is_org_agent = agent.agent_type == "org"
            tool_user_id = None if is_org_agent else agent.user_id
            tool_skip_perm = is_org_agent

            user_email = chat_user.email
            user_name = chat_user.full_name
            user_configuration = chat_user.configuration

            if not chat_messages:
                if auto_start:
                    chat_messages = []
                else:
                    raise HTTPException(
                        status_code=404, detail="No messages found for chat"
                    )

            build_result = await _build_agent_chat_registry(
                request, agent, is_admin=chat_user.role == "admin"
            )
            registry = build_result.registry
            loaded_toolsets: set[str] = set()
            pending = []
            pending_oauth = []

            run_repo = AgentRunRepository()
            runs = await run_repo.list_runs(agent.id, limit=20)
            active_sources = [
                s for s in build_result.sources if s.is_active and not s.is_deleted
            ]

            memory_provider = request.app.state.memory_provider
            effective_mode = MemoryMode.OFF
            memories = []
            if memory_provider is not None:
                config_repo = ConfigurationRepository()
                org_default = (
                    await config_repo.get_global_configuration()
                ).memory_mode_default
                if is_org_agent:
                    effective_mode = org_default
                elif user_configuration is not None:
                    effective_mode = resolve_memory_mode(
                        user_configuration.memory_mode, org_default
                    )
                memory_namespace = agent_key(agent.id)
                if effective_mode >= MemoryMode.CHAT and chat_messages:
                    last_user_text = ""
                    for msg in reversed(chat_messages):
                        m = msg.message
                        if m.get("role") == "user":
                            content = m.get("content", "")
                            if isinstance(content, str):
                                last_user_text = content
                            elif isinstance(content, list):
                                last_user_text = " ".join(
                                    b.get("text", "")
                                    for b in content
                                    if isinstance(b, dict) and b.get("type") == "text"
                                )
                            break
                    if last_user_text:
                        hits = await memory_provider.search(
                            query=last_user_text, key=memory_namespace, limit=5
                        )
                        memories = [h.record.text for h in hits if h.record.text]

            system_prompt = build_agent_chat_system_prompt(
                agent,
                runs,
                active_sources,
                user_name=user_name,
                user_email=user_email,
                memories=memories if memories else None,
                user_configuration=user_configuration,
                include_web_search=getattr(request.app.state, "web_search_provider", None)
                is not None,
                include_fetch_web_page=getattr(
                    request.app.state, "web_fetch_provider", None
                )
                is not None,
            )

            messages: list[MessageParam] = [
                MessageParam(**msg.message) for msg in chat_messages
            ]
            needs_start = not messages or messages[-1].get("role") != "user"
            if auto_start and needs_start:
                messages.append(MessageParam(role="user", content="Go."))

        else:
            # ---- Regular chat setup ----
            tool_user_id = chat.user_id
            tool_skip_perm = False
            user_email: str | None = None
            user_name: str | None = None
            user_configuration: UserConfiguration | None = None
            is_admin = False
            if chat.user_id:
                users_repo = UsersRepository()
                user = await users_repo.find_by_id(chat.user_id)
                if user:
                    user_email = user.email
                    user_name = user.full_name
                    user_configuration = user.configuration
                    is_admin = user.role == "admin"

            if not chat_messages:
                raise HTTPException(status_code=404, detail="No messages found for chat")

            messages = [MessageParam(**msg.message) for msg in chat_messages]

            loaded_toolsets = set()

            build_result = await _build_registry(
                request,
                chat,
                is_admin=is_admin,
                loaded_toolsets=loaded_toolsets,
            )
            if build_result.connector_handler is not None:
                loaded_toolsets.update(
                    _loaded_tools_from_history(messages, build_result.connector_handler)
                )
            registry = build_result.registry

            active_tool_call_ids_set = {
                tool_use["id"]
                for message in messages
                for tool_use in tool_use_blocks(message)
            }
            pending = await approvals_repo.list_for_chat(
                chat_id=chat_id,
                approval_type=ToolApprovalType.APPROVAL,
                statuses={
                    ToolApprovalStatus.PENDING,
                    ToolApprovalStatus.APPROVED,
                    ToolApprovalStatus.DENIED,
                },
                active_tool_call_ids=active_tool_call_ids_set,
            )
            pending_oauth = await approvals_repo.list_for_chat(
                chat_id=chat_id,
                approval_type=ToolApprovalType.OAUTH,
                statuses={ToolApprovalStatus.PENDING},
                active_tool_call_ids=active_tool_call_ids_set,
            )

            active_sources = [
                s for s in build_result.sources if s.is_active and not s.is_deleted
            ]

            memory_provider = request.app.state.memory_provider
            memories = []
            effective_mode = MemoryMode.OFF
            if memory_provider is not None and chat.user_id:
                memory_write_key = user_key(chat.user_id)
                config_repo = ConfigurationRepository()
                org_default = (
                    await config_repo.get_global_configuration()
                ).memory_mode_default
                user_memory_mode = (
                    user_configuration.memory_mode if user_configuration else None
                )
                effective_mode = resolve_memory_mode(user_memory_mode, org_default)
                if effective_mode >= MemoryMode.CHAT:
                    last_user_text = ""
                    for msg in reversed(chat_messages):
                        m = msg.message
                        if m.get("role") == "user":
                            content = m.get("content", "")
                            if isinstance(content, str):
                                last_user_text = content
                            elif isinstance(content, list):
                                last_user_text = " ".join(
                                    b.get("text", "")
                                    for b in content
                                    if isinstance(b, dict) and b.get("type") == "text"
                                )
                            break
                    if last_user_text:
                        hits = await memory_provider.search(
                            query=last_user_text,
                            key=user_key(chat.user_id),
                            limit=5,
                        )
                        memories = [h.record.text for h in hits if h.record.text]

            loaded_source_ids = _loaded_source_ids(
                loaded_toolsets, build_result.connector_handler
            )
            system_prompt = build_chat_system_prompt(
                active_sources,
                toolsets=build_result.toolsets,
                loaded_source_ids=loaded_source_ids,
                user_name=user_name,
                user_email=user_email,
                memories=memories if memories else None,
                user_configuration=user_configuration,
                include_web_search=getattr(request.app.state, "web_search_provider", None)
                is not None,
                include_fetch_web_page=getattr(
                    request.app.state, "web_fetch_provider", None
                )
                is not None,
            )

        # ---- Common setup (repair, compaction, etc.) ----
        if not pending and not pending_oauth:
            messages, repaired_tool_calls = repair_interrupted_tool_calls(messages)
            if repaired_tool_calls:
                logger.warning(
                    f"Inserted {repaired_tool_calls} failed tool_result placeholder(s) for interrupted tool calls in chat {chat_id}"
                )
            before = len(messages)
            messages = drop_empty_assistant_messages(messages)
            dropped = before - len(messages)
            if dropped:
                logger.warning(
                    f"Dropped {dropped} empty assistant message(s) from history for chat {chat_id}"
                )

        storage = request.app.state.content_storage
        if storage is not None:
            messages = await expand_uploads(
                messages,
                chat_id=chat_id,
                storage=storage,
                uploads_repo=UploadsRepository(),
                sandbox_url=SANDBOX_URL,
            )

        last_message_role = messages[-1].get("role") if messages else None
        if not pending and not pending_oauth and last_message_role != "user":
            logger.info(
                f"Last message is not from user, no processing needed. Chat ID: {chat_id}"
            )

            async def empty_generator():
                yield end_of_stream(
                    EndOfStreamReason.NO_NEW_MESSAGE,
                    message="No new user message to process.",
                ).encode()

            return StreamingResponse(
                empty_generator(),
                media_type="text/event-stream",
                headers={"Cache-Control": "no-cache", "Connection": "keep-alive"},
            )

        # Compaction
        secondary_provider = _resolve_secondary_provider(request.app.state)

        def _on_compaction_usage(usage):
            track_usage(
                UsageRepository(),
                UsageContext(
                    user_id=chat.user_id,
                    model_id=secondary_provider.model_record_id,
                    model_name=secondary_provider.model_name,
                    provider_type=secondary_provider.provider_type,
                    purpose=UsagePurpose.COMPACTION,
                    chat_id=chat_id,
                ),
                input_tokens=usage.input_tokens,
                output_tokens=usage.output_tokens,
                cache_read_tokens=usage.cache_read_tokens,
                cache_creation_tokens=usage.cache_creation_tokens,
            )

        compactor = ConversationCompactor(
            llm_provider=secondary_provider,
            on_usage=_on_compaction_usage,
        )
        initial_tools = build_turn_tools(
            build_result.always_on_handlers,
            build_result.connector_handler,
            loaded_toolsets,
        )

        prepared = await compactor.prepare_chat_conversation(
            chat_id=chat_id,
            chat_messages=chat_messages,
            messages=messages,
            compactions_repo=CompactionsRepository(),
            target_provider=llm_provider,
            tools=initial_tools,
            system_prompt=system_prompt,
            max_output_tokens=DEFAULT_MAX_TOKENS,
        )
        messages = prepared.messages
        latest_compaction = prepared.latest_compaction
        summarizer_context = prepared.summarizer_context
        logger.info(
            "Resolved context windows for chat %s: model=%s (%s), summarizer=%s (%s)",
            chat_id,
            prepared.model_context.tokens,
            prepared.model_context.source,
            summarizer_context.tokens,
            summarizer_context.source,
        )

        # Extract first user message for caching
        original_user_query = None
        for msg in messages:
            if msg.get("role") == "user":
                content = msg.get("content", "")
                if isinstance(content, str):
                    original_user_query = content
                    break
                elif isinstance(content, list):
                    text_parts = [
                        block.get("text", "")
                        for block in content
                        if isinstance(block, dict) and block.get("type") == "text"
                    ]
                    if text_parts:
                        original_user_query = " ".join(text_parts)
                        break

        parent_id = chat_messages[-1].id if chat_messages else None

        # ---- Build and run the generator ----
        gen = stream_generator(
            chat_id,
            redis_client,
            messages,
            llm_provider,
            chat.user_id,
            tool_user_id=tool_user_id,
            user_email=user_email,
            user_configuration=user_configuration,
            tool_skip_perm=tool_skip_perm,
            system_prompt=system_prompt,
            registry=registry,
            always_on_handlers=build_result.always_on_handlers,
            connector_handler=build_result.connector_handler,
            loaded_toolsets=loaded_toolsets,
            compactor=compactor,
            latest_compaction_summary=latest_compaction.summary if latest_compaction else None,
            summarizer_context_window_tokens=summarizer_context.tokens,
            memory_provider=memory_provider,
            memory_write_key=memory_write_key,
            effective_mode=effective_mode,
            approvals_repo=approvals_repo,
            pending=pending,
            pending_oauth=pending_oauth,
            original_user_query=original_user_query,
        )

        if redis_client is None:
            return StreamingResponse(
                persist_and_transform(gen, chat_id, messages_repo, parent_id),
                media_type="text/event-stream",
                headers=SSE_HEADERS,
            )

        # Single producer per chat
        got_lock = await redis_client.set(
            run_lock_key(chat_id), "1", nx=True, ex=_RUN_LOCK_TTL
        )
        if not got_lock:
            return StreamingResponse(
                consume_run(redis_client, chat_id, last_event_id or "0"),
                media_type="text/event-stream",
                headers=SSE_HEADERS,
            )

        await redis_client.delete(stream_key(chat_id))
        await redis_client.delete(cancel_key(chat_id))
        task = asyncio.create_task(
            run_producer(redis_client, chat_id, gen, messages_repo, parent_id)
        )
        set_producer_task(chat_id, task)

        def _cleanup_run_task(
            t: asyncio.Task, cid: str = chat_id  # type: ignore[assignment]
        ) -> None:
            if _run_tasks_by_chat.get(cid) is t:
                clear_producer_task(cid, t)

        task.add_done_callback(_cleanup_run_task)

        return StreamingResponse(
            consume_run(redis_client, chat_id, "0"),
            media_type="text/event-stream",
            headers=SSE_HEADERS,
        )


# ---------------------------------------------------------------------------
# Route: stream chat
# ---------------------------------------------------------------------------

@router.get("/chat/{chat_id}/stream")
async def stream_chat(
    request: Request,
    chat_id: str = Path(..., description="Chat thread ID"),
    auto_start: bool = Query(
        False, description="Auto-inject initial message for agent chats"
    ),
):
    return await StreamChatHandler(request, chat_id, auto_start).handle()


# Route: cancel
# ---------------------------------------------------------------------------


@router.post("/chat/{chat_id}/cancel")
async def cancel_chat_stream(
    request: Request, chat_id: str = Path(..., description="Chat thread ID")
):
    redis_client = request.app.state.redis_client
    try:
        await redis_client.set(cancel_key(chat_id), "1", ex=_CANCEL_TTL)
    except Exception as e:
        logger.error(f"Failed to set cancel flag for chat {chat_id}: {e}")

    task = _run_tasks_by_chat.get(chat_id)
    if task is not None and not task.done():
        # Best-effort immediate stop for this worker. The Redis cancel flag above
        # is the cross-worker mechanism when Stop lands on a different process.
        task.cancel()

    return {"status": "cancelling"}


# ---------------------------------------------------------------------------
# Route: generate title
# ---------------------------------------------------------------------------


@router.post("/chat/{chat_id}/generate_title")
async def generate_chat_title(
    request: Request, chat_id: str = Path(..., description="Chat thread ID")
):
    logger.info(f"Generating title for chat: {chat_id}")

    try:
        chats_repo = ChatsRepository()
        chat = await chats_repo.get(chat_id)
        if not chat:
            raise HTTPException(status_code=404, detail="Chat thread not found")

        llm_provider = _resolve_secondary_provider(request.app.state)

        if chat.title:
            logger.info(f"Chat already has a title: {chat.title}")
            return {"title": chat.title, "status": "existing"}

        messages_repo = MessagesRepository()
        chat_messages = await messages_repo.get_by_chat(chat_id)
        if not chat_messages:
            raise HTTPException(
                status_code=400, detail="Not enough messages to generate title"
            )

        conversation_text = ""
        for msg in chat_messages:
            role = msg.message.get("role", "unknown")
            if role != "user":
                continue
            content = msg.message.get("content")
            text = _extract_text_for_title(content)
            if text is not None:
                conversation_text = f"User: {text}\n"
                break

        if not conversation_text.strip():
            logger.info(
                "Skipping title generation; no user text found",
                extra={"chat_id": chat_id},
            )
            return {"status": "skipped", "reason": "no_user_text"}

        logger.info(f"Extracted conversation text ({len(conversation_text)} chars)")

        title_result = await generate_title_for_conversation(
            llm_provider,
            conversation_text,
            chat_id,
        )
        title = title_result.title

        if title_result.usage is not None:
            track_usage(
                UsageRepository(),
                UsageContext(
                    user_id=chat.user_id,
                    model_id=llm_provider.model_record_id,
                    model_name=llm_provider.model_name,
                    provider_type=llm_provider.provider_type,
                    purpose=UsagePurpose.TITLE_GENERATION,
                    chat_id=chat_id,
                ),
                input_tokens=title_result.usage.input_tokens,
                output_tokens=title_result.usage.output_tokens,
                cache_read_tokens=title_result.usage.cache_read_tokens,
                cache_creation_tokens=title_result.usage.cache_creation_tokens,
            )

        logger.info(f"Generated title: {title}")

        updated_chat = await chats_repo.update_title(chat_id, title)
        if not updated_chat:
            raise HTTPException(status_code=500, detail="Failed to update chat title")

        return {"title": title, "status": "generated"}

    except HTTPException:
        raise
    except Exception as e:
        logger.error(
            f"Failed to generate title for chat {chat_id}: {e}",
            exc_info=True,
        )
        raise HTTPException(
            status_code=500, detail=f"Failed to generate title: {str(e)}"
        )


# ---------------------------------------------------------------------------
# Route: download artifact
# ---------------------------------------------------------------------------


@router.get("/chat/{chat_id}/artifacts/{path:path}")
async def download_artifact(
    request: Request,
    chat_id: str = Path(..., description="Chat thread ID"),
    path: str = Path(..., description="Relative file path in the sandbox"),
):
    try:
        async with httpx.AsyncClient(timeout=30.0) as client:
            resp = await client.get(
                f"{SANDBOX_URL}/files/download",
                params={"chat_id": chat_id, "path": path},
            )

            if resp.status_code == 404:
                raise HTTPException(status_code=404, detail="Artifact not found")

            resp.raise_for_status()

            content_type = resp.headers.get("content-type", "application/octet-stream")
            return Response(
                content=resp.content,
                media_type=content_type,
                headers={"Cache-Control": "private, max-age=3600"},
            )
    except httpx.HTTPStatusError as e:
        logger.error(f"Sandbox artifact download failed: {e}")
        raise HTTPException(
            status_code=502, detail="Failed to fetch artifact from sandbox"
        )
    except Exception as e:
        logger.error(f"Artifact download error: {e}")
        raise HTTPException(status_code=500, detail="Internal error fetching artifact")


# Re-exports for backward compatibility with tests.
# The canonical homes are in ``services/ai/streaming/``.
from streaming.persist import partial_assistant_message as _partial_assistant_message  # noqa: E402, F401
from streaming.run import _run_tasks_by_chat  # noqa: E402, F401
