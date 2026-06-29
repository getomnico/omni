import asyncio
import json
import logging
import pathlib
from collections.abc import Iterable
from dataclasses import dataclass
from typing import cast

import httpx
from anthropic import AsyncStream, MessageStreamEvent
from anthropic.types import (
    CitationCharLocationParam,
    CitationContentBlockLocationParam,
    CitationPageLocationParam,
    CitationsDelta,
    CitationSearchResultLocationParam,
    CitationWebSearchResultLocationParam,
    ContentBlockParam,
    MessageParam,
    TextBlockParam,
    TextCitationParam,
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
    APPROVAL_TIMEOUT_SECONDS,
    CONNECTOR_MANAGER_URL,
    DEFAULT_MAX_TOKENS,
    DEFAULT_TEMPERATURE,
    DEFAULT_TOP_P,
    SANDBOX_URL,
)
from db import ChatsRepository, MessagesRepository
from db.documents import DocumentsRepository
from db.configuration import ConfigurationRepository
from db.models import Chat, Source, UserConfiguration
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
from providers import LLMProvider, LLMProviderStreamError
from providers import ProviderError
from services.compaction import ConversationCompactor
from services.title_generation import generate_title_for_conversation
from services.usage import UsageContext, UsagePurpose, UsageTracker, track_usage
from state import AppState
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
from tools.omni_tool_result import OAuthRequiredPayload
from tools.sandbox_handler import SandboxToolHandler
from tools.search_handler import fetch_operator_values
from tools.skill_handler import SkillHandler
from tools.turn_builder import build_turn_tools

router = APIRouter(tags=["chat"])
logger = logging.getLogger(__name__)


def _chat_error_message(exc: Exception) -> str:
    if isinstance(exc, ProviderError) and exc.message:
        return f"Failed to generate response: {exc.message}"

    if isinstance(exc, LLMProviderStreamError) and exc.message:
        return f"Failed to generate response: {exc.message}"

    message = str(exc).strip()
    if message:
        return f"Failed to generate response: {message}"

    return "Failed to generate response. Please try again."


def _chat_error_payload(exc: Exception) -> dict[str, object]:
    payload: dict[str, object] = {"message": _chat_error_message(exc)}
    if isinstance(exc, ProviderError):
        payload["provider"] = exc.provider_type
        payload["model"] = exc.model
        payload["statusCode"] = exc.status_code
    return payload


def _sse_event(event_type: str, data: object) -> str:
    return f"event: {event_type}\ndata: {json.dumps(data)}\n\n"


# --- Decoupled streaming run (survives client disconnect; supports resume) ---
# The agent loop runs as a background "producer" that writes each SSE event into
# a per-chat Redis Stream. HTTP requests are thin "consumers" that tail that
# stream from an offset (SSE Last-Event-ID), so a client that backgrounds/
# reconnects resumes without interrupting generation. The producer is the single
# DB writer of the streaming path (persists messages, then emits `message_id`),
# which keeps replays free of duplicate writes.

SSE_HEADERS = {"Cache-Control": "no-cache", "Connection": "keep-alive"}

_STREAM_HEARTBEAT_MS = 15000  # idle ping interval (keeps proxies from timing out)
_RUN_LOCK_TTL = (
    300  # seconds; refreshed on every produced event and by the heartbeat below
)
_LOCK_REFRESH_INTERVAL = 60  # seconds; independent of event production, so a long
# silent gap in the agent loop (e.g. a slow tool call with no intermediate SSE
# events) can't let the lock expire while the producer is still running.
_STREAM_TTL = 300  # seconds a finished stream stays replayable
_STREAM_MAXLEN = 5000  # cap buffered events per run
_CANCEL_TTL = 300
_CANCEL_CHECK_INTERVAL_SECONDS = 1.0

_background_run_tasks: set[asyncio.Task] = set()


def _stream_key(chat_id: str) -> str:
    return f"chat:stream:{chat_id}"


def _run_lock_key(chat_id: str) -> str:
    return f"chat:runlock:{chat_id}"


def _cancel_key(chat_id: str) -> str:
    return f"chat:cancel:{chat_id}"


async def _is_run_cancelled(redis_client, chat_id: str) -> bool:
    if redis_client is None:
        return False
    try:
        return bool(await redis_client.exists(_cancel_key(chat_id)))
    except Exception:
        return False


def _sse_event_type(event_str: str) -> str:
    for line in event_str.split("\n"):
        if line.startswith("event:"):
            return line[len("event:") :].strip()
    return "message"


def _sse_event_data(event_str: str) -> str:
    for line in event_str.split("\n"):
        if line.startswith("data:"):
            return line[len("data:") :].strip()
    return ""


def _partial_assistant_message(
    content_blocks: list[TextBlockParam | ToolUseBlockParam],
) -> MessageParam | None:
    text_blocks: list[TextBlockParam] = []
    for block in content_blocks:
        if block["type"] != "text":
            continue
        text_block = cast(TextBlockParam, block)
        if not text_block["text"].strip():
            continue
        text_blocks.append(cast(TextBlockParam, dict(text_block)))

    if not text_blocks:
        return None

    return MessageParam(role="assistant", content=text_blocks)


async def _persist_and_transform(gen, chat_id, messages_repo, parent_id):
    """Persist assistant/tool_result messages (single writer of the streaming
    path) and replace each internal `save_message` event with a client-facing
    `message_id` event. Deterministic parent_id chaining makes replays safe."""
    async for event_str in gen:
        if _sse_event_type(event_str) == "save_message":
            try:
                message = json.loads(_sse_event_data(event_str))
                created = await messages_repo.create(
                    chat_id, message, parent_id=parent_id
                )
                parent_id = created.id
                yield f"event: message_id\ndata: {created.id}\n\n"
            except Exception as e:
                logger.error(
                    f"Failed to persist streamed message for chat {chat_id}: {e}"
                )
            continue
        yield event_str


async def _refresh_lock_periodically(redis_client, lock_key):
    """Keep the run lock alive independently of event production, so a long
    silent gap in the agent loop doesn't let it expire mid-run."""
    while True:
        await asyncio.sleep(_LOCK_REFRESH_INTERVAL)
        await redis_client.expire(lock_key, _RUN_LOCK_TTL)


async def _run_producer(redis_client, chat_id, gen, messages_repo, parent_id):
    """Background task: drive the agent loop to completion independently of any
    client connection, buffering every SSE event in a Redis Stream."""
    stream_key = _stream_key(chat_id)
    lock_key = _run_lock_key(chat_id)
    refresh_task = asyncio.create_task(
        _refresh_lock_periodically(redis_client, lock_key)
    )
    try:
        async for event_str in _persist_and_transform(
            gen, chat_id, messages_repo, parent_id
        ):
            await redis_client.xadd(
                stream_key,
                {"e": event_str},
                maxlen=_STREAM_MAXLEN,
                approximate=True,
            )
            await redis_client.expire(lock_key, _RUN_LOCK_TTL)
    except Exception as e:
        logger.error(f"Producer failed for chat {chat_id}: {e}", exc_info=True)
        try:
            await redis_client.xadd(
                stream_key,
                {"e": _sse_event("stream_error", {"message": _chat_error_message(e)})},
            )
        except Exception:
            pass
    finally:
        refresh_task.cancel()
        try:
            await refresh_task
        except asyncio.CancelledError:
            pass
        for coro in (
            redis_client.expire(stream_key, _STREAM_TTL),
            redis_client.delete(lock_key),
            redis_client.delete(_cancel_key(chat_id)),
        ):
            try:
                await coro
            except Exception:
                pass


async def _consume_run(redis_client, chat_id, start_id):
    """Thin consumer: tail the Redis Stream from `start_id`, prefixing each event
    with its Redis id (SSE `id:`) for Last-Event-ID resume. Emits heartbeats
    while idle and a terminal event if the producer vanished."""
    stream_key = _stream_key(chat_id)
    lock_key = _run_lock_key(chat_id)
    last = start_id or "0"
    while True:
        resp = await redis_client.xread(
            {stream_key: last}, block=_STREAM_HEARTBEAT_MS, count=200
        )
        if resp:
            for _key, entries in resp:
                for entry_id, fields in entries:
                    last = entry_id
                    event_str = fields.get("e", "")
                    yield f"id: {entry_id}\n{event_str}"
                    if _sse_event_type(event_str) in ("end_of_stream", "stream_error"):
                        return
            continue
        # Idle: no new events within the heartbeat window.
        if not await redis_client.exists(stream_key):
            if await redis_client.exists(lock_key):
                # Producer just started and hasn't written its first event yet.
                yield _sse_event("heartbeat", {})
                continue
            yield "event: not_resumable\ndata: \n\n"
            return
        if not await redis_client.exists(lock_key):
            # Producer is gone but never wrote a terminal event we forwarded.
            yield _sse_event(
                "stream_error", {"message": "Generation ended unexpectedly."}
            )
            return
        yield _sse_event("heartbeat", {})


def _resolve_provider(state: AppState, model_id: str | None) -> LLMProvider:
    """Resolve a model by ID, returning the provider.
    Priority: requested model -> default model -> first available.
    """
    models = state.models
    if not models:
        raise HTTPException(status_code=503, detail="No models configured")

    if model_id and model_id in models:
        return models[model_id]
    if state.default_model_id and state.default_model_id in models:
        return models[state.default_model_id]
    return next(iter(models.values()))


def _resolve_llm_provider(state: AppState, chat: Chat) -> LLMProvider:
    """Resolve which LLM provider to use for a chat."""
    return _resolve_provider(state, chat.model_id)


def _resolve_secondary_provider(state: AppState) -> LLMProvider:
    """Resolve the secondary (lightweight) model provider.
    Used for title generation, suggested questions, compaction, etc.
    """
    return _resolve_provider(state, state.secondary_model_id or state.default_model_id)


def convert_citation_to_param(citation_delta: CitationsDelta) -> TextCitationParam:
    citation = citation_delta.citation
    if citation.type == "char_location":
        return CitationCharLocationParam(
            type="char_location",
            start_char_index=citation.start_char_index,
            end_char_index=citation.end_char_index,
            document_title=citation.document_title,
            document_index=citation.document_index,
            cited_text=citation.cited_text,
        )
    elif citation.type == "page_location":
        return CitationPageLocationParam(
            type="page_location",
            start_page_number=citation.start_page_number,
            end_page_number=citation.end_page_number,
            document_title=citation.document_title,
            document_index=citation.document_index,
            cited_text=citation.cited_text,
        )
    elif citation.type == "content_block_location":
        return CitationContentBlockLocationParam(
            type="content_block_location",
            start_block_index=citation.start_block_index,
            end_block_index=citation.end_block_index,
            document_title=citation.document_title,
            document_index=citation.document_index,
            cited_text=citation.cited_text,
        )
    elif citation.type == "search_result_location":
        return CitationSearchResultLocationParam(
            type="search_result_location",
            start_block_index=citation.start_block_index,
            end_block_index=citation.end_block_index,
            search_result_index=citation.search_result_index,
            title=citation.title,
            source=citation.source,
            cited_text=citation.cited_text,
        )
    elif citation.type == "web_search_result_location":
        return CitationWebSearchResultLocationParam(
            type="web_search_result_location",
            url=citation.url,
            title=citation.title,
            encrypted_index=citation.encrypted_index,
            cited_text=citation.cited_text,
        )
    else:
        raise ValueError(f"Unknown citation type: {citation.type}")


@dataclass
class RegistryResult:
    registry: ToolRegistry
    # Handlers whose tools are always exposed in the LLM's per-turn tool list
    # (built-ins + meta-tools). Connector tools are handled separately.
    always_on_handlers: list[ToolHandler]
    connector_handler: ConnectorToolHandler | None
    toolsets: list[ToolsetSummary]
    sources: list[Source]
    search_operators: list[SearchOperator]


def _loaded_tools_from_history(
    messages: list[MessageParam], connector_handler: ConnectorToolHandler
) -> set[str]:
    """Rebuild loaded connector tool names from successful meta-tool calls."""
    tool_calls: dict[str, ToolUseBlockParam] = {}
    loaded: set[str] = set()

    for message in messages:
        for block in _message_content_blocks(message):
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


def _message_content_blocks(message: MessageParam) -> list[ContentBlockParam]:
    content = message["content"]
    if isinstance(content, str):
        return []
    return list(cast(Iterable[ContentBlockParam], content))


def _message_has_tool_use(message: dict, tool_use_id: str) -> bool:
    content = message.get("content")
    if not isinstance(content, list):
        return False
    return any(
        isinstance(block, dict)
        and block.get("type") == "tool_use"
        and block.get("id") == tool_use_id
        for block in content
    )


async def _find_tool_use_message_id(
    messages_repo: MessagesRepository, chat_id: str, tool_use_id: str
) -> str | None:
    for chat_message in await messages_repo.get_by_chat(chat_id):
        if _message_has_tool_use(chat_message.message, tool_use_id):
            return chat_message.id
    return None


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


def _copy_provider_extras(src: object, dst: dict, keys: tuple[str, ...]) -> None:
    """Copy provider-declared sidecar fields off a Pydantic content_block
    instance onto its persisted TypedDict block.

    The set of keys is owned by each ``LLMProvider`` subclass via
    ``PERSISTED_BLOCK_EXTRAS`` — chat.py stays provider-agnostic.
    """
    for key in keys:
        value = getattr(src, key, None)
        if value is not None:
            dst[key] = value  # type: ignore[typeddict-unknown-key]


async def _fetch_sources_from_connector_manager() -> list[Source] | None:
    """Fetch all sources from the connector manager. Returns None on failure."""
    if not CONNECTOR_MANAGER_URL:
        return None
    try:
        async with httpx.AsyncClient(timeout=10.0) as client:
            resp = await client.get(f"{CONNECTOR_MANAGER_URL.rstrip('/')}/sources")
            resp.raise_for_status()
            return sources_from_sync_overview_response(resp.json())
    except Exception as e:
        logger.warning(f"Failed to fetch sources from connector manager: {e}")
        return None


async def _build_registry(
    request: Request,
    chat: Chat,
    is_admin: bool,
    loaded_toolsets: set[str],
    on_load: OnLoad | None = None,
) -> RegistryResult:
    """Build a ToolRegistry with all available handlers.

    Connector tools are registered for *dispatch* but not for *exposure*: the
    chat router rebuilds the per-turn LLM tool list from `always_on_handlers`
    plus `connector_handler.filtered_tools(loaded_toolsets)` so the model only
    sees connector actions it has explicitly loaded via `load_tool` or
    `load_tool_set` (issue #203).
    """
    registry = ToolRegistry()
    always_on_handlers: list[ToolHandler] = []

    # Fetch sources from connector manager once, share with all handlers
    sources = await _fetch_sources_from_connector_manager() or []

    connector_handler: ConnectorToolHandler | None = None
    toolsets: list[ToolsetSummary] = []
    search_operators: list[SearchOperator] = []

    # Register connector tools if connector-manager is configured
    if CONNECTOR_MANAGER_URL:
        connector_handler = ConnectorToolHandler(
            connector_manager_url=CONNECTOR_MANAGER_URL,
            user_id=chat.user_id,
            redis_client=getattr(request.app.state, "redis_client", None),
            prefetched_sources=sources,
            documents_repo=DocumentsRepository(),
            sandbox_url=SANDBOX_URL,
            is_admin=is_admin,
        )
        await connector_handler._ensure_initialized()
        # Register for dispatch / requires_approval; tool exposure is filtered per-turn.
        registry.register(connector_handler)

        if connector_handler.actions:
            toolsets = connector_handler.list_toolsets()

        if connector_handler.search_operators:
            search_operators = connector_handler.search_operators

    # Meta-tools: discoverable as always-on so the LLM can opt in to specific
    # connector tools. Only register when there's actually a connector handler
    # with toolsets to advertise.
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

    # Fetch dynamic operator values for enriched search tool description
    active_sources = [s for s in sources if s.is_active and not s.is_deleted]
    connected_source_types = list({s.source_type for s in active_sources})
    operator_values: dict[str, list[str]] = {}
    if search_operators:
        operator_values = await fetch_operator_values(
            request.app.state.searcher_tool.client,
            search_operators,
            redis_client=getattr(request.app.state, "redis_client", None),
        )

    # Register search tools (with dynamic operators from connector manifests)
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

    # Register people search tool
    people_handler = PeopleSearchHandler(searcher_tool=request.app.state.searcher_tool)
    registry.register(people_handler)
    always_on_handlers.append(people_handler)

    # Register document handler (unified read_document tool)
    content_storage = getattr(request.app.state, "content_storage", None)
    if content_storage or CONNECTOR_MANAGER_URL:
        document_handler = DocumentToolHandler(
            content_storage=content_storage,
            documents_repo=DocumentsRepository(),
            sandbox_url=SANDBOX_URL,
            connector_manager_url=CONNECTOR_MANAGER_URL or None,
        )
        registry.register(document_handler)
        always_on_handlers.append(document_handler)

    # Register sandbox tools if sandbox service is configured
    if SANDBOX_URL:
        sandbox_handler = SandboxToolHandler(sandbox_url=SANDBOX_URL)
        registry.register(sandbox_handler)
        always_on_handlers.append(sandbox_handler)

    # Register skill loader (load_skill tool)
    skills_dir = pathlib.Path(__file__).resolve().parent.parent / "skills"
    skill_handler = SkillHandler(
        skills_dir=skills_dir,
        searcher_client=request.app.state.searcher_tool.client,
        connector_manager_url=CONNECTOR_MANAGER_URL or None,
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


async def _noop_on_load(_: set[str]) -> None:
    """Default on_load when no persistence is wired up (e.g. agent chat)."""
    return None


async def _build_agent_chat_registry(
    request: Request, agent: Agent, is_admin: bool
) -> RegistryResult:
    """Build a read-only ToolRegistry for agent chat sessions.

    Uses the agent's own permissions (matching the background executor):
    org agents read across everything; user agents are scoped by allowed_sources.
    Write/connector-action tools are intentionally not registered — agent chats are read-only.
    """
    registry = ToolRegistry()
    always_on_handlers: list[ToolHandler] = []

    sources = await _fetch_sources_from_connector_manager() or []

    source_filter = _build_source_filter(agent) if agent.agent_type == "user" else None

    # We still need connector handler for search operators, but won't register it
    search_operators: list[SearchOperator] = []
    if CONNECTOR_MANAGER_URL:
        connector_handler = ConnectorToolHandler(
            connector_manager_url=CONNECTOR_MANAGER_URL,
            user_id=agent.user_id if agent.agent_type == "user" else "",
            redis_client=getattr(request.app.state, "redis_client", None),
            prefetched_sources=sources,
            source_filter=source_filter,
            documents_repo=DocumentsRepository(),
            is_admin=is_admin,
        )
        await connector_handler._ensure_initialized()
        if connector_handler.search_operators:
            search_operators = connector_handler.search_operators

    active_sources = [s for s in sources if s.is_active and not s.is_deleted]
    connected_source_types = list({s.source_type for s in active_sources})
    operator_values: dict[str, list[str]] = {}
    if search_operators:
        operator_values = await fetch_operator_values(
            request.app.state.searcher_tool.client,
            search_operators,
            redis_client=getattr(request.app.state, "redis_client", None),
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
    if content_storage or CONNECTOR_MANAGER_URL:
        document_handler = DocumentToolHandler(
            content_storage=content_storage,
            documents_repo=DocumentsRepository(),
            sandbox_url=SANDBOX_URL,
            connector_manager_url=CONNECTOR_MANAGER_URL or None,
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
        connector_manager_url=CONNECTOR_MANAGER_URL or None,
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


async def _save_pending_approval(
    redis_client,
    chat_id: str,
    tool_call: dict,
    conversation_messages: list[MessageParam],
    action_info: dict | None = None,
) -> str:
    """Save pending approval state to Redis."""
    import ulid

    approval_id = str(ulid.ULID())
    state = {
        "approval_id": approval_id,
        "tool_call": {
            "id": tool_call["id"],
            "name": tool_call["name"],
            "input": tool_call["input"],
        },
        "conversation_messages": conversation_messages,
        "source_id": action_info.get("source_id") if action_info else None,
        "source_type": action_info.get("source_type") if action_info else None,
        "action_name": action_info.get("action_name") if action_info else None,
    }

    key = f"chat:{chat_id}:pending_approval"
    await redis_client.set(
        key, json.dumps(state, default=str), ex=APPROVAL_TIMEOUT_SECONDS
    )
    logger.info(f"Saved pending approval {approval_id} for chat {chat_id}")
    return approval_id


async def _get_pending_approval(redis_client, chat_id: str) -> dict | None:
    """Get pending approval state from Redis."""
    key = f"chat:{chat_id}:pending_approval"
    try:
        data = await redis_client.get(key)
        if data:
            return json.loads(data)
    except Exception as e:
        logger.warning(f"Failed to get pending approval: {e}")
    return None


async def _clear_pending_approval(redis_client, chat_id: str) -> None:
    """Clear pending approval state from Redis."""
    key = f"chat:{chat_id}:pending_approval"
    await redis_client.delete(key)


async def _tool_approval_status(approval_id: str | None) -> str | None:
    if not approval_id:
        return None
    try:
        pool = await MessagesRepository()._get_pool()
        async with pool.acquire() as conn:
            return await conn.fetchval(
                "SELECT status FROM tool_approvals WHERE id = $1", approval_id
            )
    except Exception as e:
        logger.warning(f"Failed to read tool approval {approval_id}: {e}")
        return None


async def _save_pending_oauth(
    redis_client,
    chat_id: str,
    tool_calls: list[dict],
    conversation_messages: list[MessageParam],
) -> None:
    """Save pending OAuth state to Redis."""
    state = {
        "tool_calls": [
            {"id": tc["id"], "name": tc["name"], "input": tc["input"]}
            for tc in tool_calls
        ],
        "conversation_messages": conversation_messages,
    }
    key = f"chat:{chat_id}:pending_oauth"
    await redis_client.set(
        key, json.dumps(state, default=str), ex=APPROVAL_TIMEOUT_SECONDS
    )
    logger.info(
        f"Saved pending OAuth for chat {chat_id} ({len(tool_calls)} tool call(s))"
    )


async def _get_pending_oauth(redis_client, chat_id: str) -> dict | None:
    key = f"chat:{chat_id}:pending_oauth"
    try:
        data = await redis_client.get(key)
        if data:
            return json.loads(data)
    except Exception as e:
        logger.warning(f"Failed to get pending OAuth: {e}")
    return None


async def _clear_pending_oauth(redis_client, chat_id: str) -> None:
    key = f"chat:{chat_id}:pending_oauth"
    await redis_client.delete(key)


@router.get("/chat/{chat_id}/stream_status")
async def stream_status(
    request: Request, chat_id: str = Path(..., description="Chat thread ID")
):
    redis_client = getattr(request.app.state, "redis_client", None)
    if redis_client is None:
        return {
            "running": False,
            "resumable": False,
            "pending_approval": False,
            "pending_oauth": False,
        }

    return {
        "running": bool(await redis_client.exists(_run_lock_key(chat_id))),
        "resumable": bool(await redis_client.exists(_stream_key(chat_id))),
        "pending_approval": bool(await _get_pending_approval(redis_client, chat_id)),
        "pending_oauth": bool(await _get_pending_oauth(redis_client, chat_id)),
    }


@router.get("/chat/{chat_id}/stream")
async def stream_chat(
    request: Request,
    chat_id: str = Path(..., description="Chat thread ID"),
    auto_start: bool = Query(
        False, description="Auto-inject initial message for agent chats"
    ),
):
    """Stream AI response for a chat thread using Server-Sent Events"""
    if not request.app.state.searcher_tool:
        raise HTTPException(status_code=500, detail="Searcher tool not initialized")

    # Retrieve chat and messages from database
    chats_repo = ChatsRepository()
    chat = await chats_repo.get(chat_id)
    if not chat:
        raise HTTPException(status_code=404, detail="Chat thread not found")

    llm_provider = _resolve_llm_provider(request.app.state, chat)
    redis_client = getattr(request.app.state, "redis_client", None)

    # Reconnect/resume fast path: if a buffered run already exists for this chat,
    # attach to it (tail from the client's offset) and skip all (re)setup so we
    # never re-run registry build / compaction / tools on a reconnect.
    last_event_id = request.headers.get("last-event-id") or request.query_params.get(
        "last_event_id"
    )
    if redis_client is not None and last_event_id is not None:
        if await redis_client.exists(_stream_key(chat_id)):
            return StreamingResponse(
                _consume_run(redis_client, chat_id, last_event_id),
                media_type="text/event-stream",
                headers=SSE_HEADERS,
            )

        # Stream expired (TTL elapsed). Starting a new generation would produce a
        # duplicate response — tell the client to reload from the database instead.
        async def _not_resumable_response():
            yield "event: not_resumable\ndata: \n\n"

        return StreamingResponse(
            _not_resumable_response(),
            media_type="text/event-stream",
            headers=SSE_HEADERS,
        )

    messages_repo = MessagesRepository()
    chat_messages = await messages_repo.get_active_path(chat_id)

    # Memory state — populated in both agent and regular chat branches
    memory_provider = None
    effective_mode = MemoryMode.OFF
    memories: list[str] = []
    memory_write_key: str | None = (
        None  # None = no write (e.g. agent chats are read-only)
    )
    pending = None
    pending_oauth = None

    if chat.agent_id:
        # --- Agent chat setup ---
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
                    status_code=403, detail="Admin access required for org agent chats"
                )
        elif agent.user_id != chat.user_id:
            raise HTTPException(
                status_code=403, detail="Only the agent owner can chat with this agent"
            )

        is_org_agent = agent.agent_type == "org"
        tool_user_id = None if is_org_agent else agent.user_id
        tool_skip_perm = is_org_agent

        user_email = chat_user.email
        user_name = chat_user.full_name
        user_configuration = chat_user.configuration

        # Handle auto_start: inject ephemeral message when no messages exist
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
        # Agent chats are read-only; no connector handler, so the per-turn
        # builder collapses to just the always-on handlers.
        loaded_toolsets: set[str] = set()
        pending = None  # no approval flow for agent chats

        # Build agent chat system prompt with run history
        run_repo = AgentRunRepository()
        runs = await run_repo.list_runs(agent.id, limit=20)
        active_sources = [
            s for s in build_result.sources if s.is_active and not s.is_deleted
        ]

        # Memory: fetch agent-scoped memories (same scoping as background executor)
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

        # Build messages, injecting ephemeral start message if needed
        messages: list[MessageParam] = [
            MessageParam(**msg.message) for msg in chat_messages
        ]
        needs_start = not messages or messages[-1].get("role") != "user"
        if auto_start and needs_start:
            messages.append(MessageParam(role="user", content="Go."))

    else:
        # --- Regular chat setup ---
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

        messages: list[MessageParam] = [
            MessageParam(**msg.message) for msg in chat_messages
        ]

        # Rebuild loaded connector sources from prior meta-tool calls/results.
        # Tool discovery is part of the conversation, not chat-session state.
        loaded_toolsets: set[str] = set()

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

        # Check for pending approval / OAuth resume flow
        pending = None
        pending_oauth = None
        if redis_client:
            pending = await _get_pending_approval(redis_client, chat_id)
            pending_oauth = await _get_pending_oauth(redis_client, chat_id)

        active_sources = [
            s for s in build_result.sources if s.is_active and not s.is_deleted
        ]

        # Memory: resolve mode and fetch relevant memories
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

    # Expand any omni_upload content blocks (inline small text, stage larger/binary in sandbox).
    storage = request.app.state.content_storage
    if storage is not None:
        messages = await expand_uploads(
            messages,
            chat_id=chat_id,
            storage=storage,
            uploads_repo=UploadsRepository(),
            sandbox_url=SANDBOX_URL,
        )

    # Check if we need to process - only if last message is from user (or resuming from approval / OAuth)
    last_message_role = messages[-1].get("role") if messages else None
    if not pending and not pending_oauth and last_message_role != "user":
        logger.info(
            f"Last message is not from user, no processing needed. Chat ID: {chat_id}"
        )

        async def empty_generator():
            yield b"event: end_of_stream\ndata: No new user message to process.\n\n"

        return StreamingResponse(
            empty_generator(),
            media_type="text/event-stream",
            headers={"Cache-Control": "no-cache", "Connection": "keep-alive"},
        )

    # Check if conversation needs compaction
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
        redis_client=redis_client,
        on_usage=_on_compaction_usage,
    )
    # Compaction sees the *current* per-turn tool list — connector tools the
    # chat hasn't loaded don't count against the budget, which is the whole
    # point of lazy loading.
    initial_tools = build_turn_tools(
        build_result.always_on_handlers,
        build_result.connector_handler,
        loaded_toolsets,
    )
    if compactor.needs_compaction(messages, initial_tools):
        logger.info(f"Compacting conversation for chat {chat_id}")
        messages = await compactor.compact_conversation(chat_id, messages)

    # Stream AI response with tool calling
    async def stream_generator():
        try:
            if pending and isinstance(pending.get("conversation_messages"), list):
                conversation_messages = pending["conversation_messages"].copy()
            elif pending_oauth and isinstance(
                pending_oauth.get("conversation_messages"), list
            ):
                conversation_messages = pending_oauth["conversation_messages"].copy()
            else:
                conversation_messages = messages.copy()

            # Handle approval resume
            if pending:
                logger.info(f"Resuming from pending approval for chat {chat_id}")

                tool_call = pending["tool_call"]
                approval_id = pending.get("approval_id")
                approval_status = await _tool_approval_status(approval_id)

                if approval_status != "approved":
                    if approval_status == "denied":
                        await _clear_pending_approval(redis_client, chat_id)
                        tool_result = ToolResultBlockParam(
                            type="tool_result",
                            tool_use_id=tool_call["id"],
                            content=[
                                {
                                    "type": "text",
                                    "text": "The user denied approval for this tool call.",
                                }
                            ],
                            is_error=True,
                        )
                        yield f"event: message\ndata: {json.dumps(tool_result)}\n\n"
                        tool_result_message = MessageParam(
                            role="user", content=[tool_result]
                        )
                        conversation_messages.append(tool_result_message)
                        yield f"event: save_message\ndata: {json.dumps(tool_result_message)}\n\n"
                    else:
                        approval_event = {
                            "approval_id": approval_id,
                            "tool_name": tool_call["name"],
                            "tool_input": tool_call["input"],
                            "tool_call_id": tool_call["id"],
                        }
                        yield f"event: approval_required\ndata: {json.dumps(approval_event)}\n\n"
                        yield "event: end_of_stream\ndata: Approval required\n\n"
                        return
                else:
                    await _clear_pending_approval(redis_client, chat_id)

                    context = ToolContext(
                        chat_id=chat_id,
                        user_id=tool_user_id,
                        user_email=user_email,
                        user_configuration=user_configuration,
                        skip_permission_check=tool_skip_perm,
                    )
                    result = await registry.execute(
                        tool_call["name"], tool_call["input"], context
                    )

                    tool_result = ToolResultBlockParam(
                        type="tool_result",
                        tool_use_id=tool_call["id"],
                        content=result.content,
                        is_error=result.is_error,
                    )

                    # Emit the tool result to the client
                    yield f"event: message\ndata: {json.dumps(tool_result)}\n\n"

                    tool_result_message = MessageParam(
                        role="user", content=[tool_result]
                    )
                    conversation_messages.append(tool_result_message)
                    yield f"event: save_message\ndata: {json.dumps(tool_result_message)}\n\n"

            # Handle OAuth resume — replace each placeholder tool_result in-place
            # so the LLM sees real results on the next iteration, not the
            # `omni_kind: oauth_required` envelopes we wrote at pause time.
            # The pending state may carry multiple tool calls when the model
            # fanned out parallel calls against the same source; replace them
            # all so the LLM doesn't see a mix of real and stale results.
            elif pending_oauth:
                logger.info(f"Resuming from pending OAuth for chat {chat_id}")
                await _clear_pending_oauth(redis_client, chat_id)

                pending_tool_calls = pending_oauth["tool_calls"]

                # Build tool_use_id -> chat_messages.id map by walking the
                # persisted message rows once.
                placeholder_ids: dict[str, str] = {}
                pending_ids = {tc["id"] for tc in pending_tool_calls}
                for cm in chat_messages:
                    msg = cm.message
                    if not isinstance(msg, dict) or msg.get("role") != "user":
                        continue
                    content = msg.get("content")
                    if not isinstance(content, list):
                        continue
                    for block in content:
                        if (
                            isinstance(block, dict)
                            and block.get("type") == "tool_result"
                            and block.get("tool_use_id") in pending_ids
                            and block["tool_use_id"] not in placeholder_ids
                        ):
                            placeholder_ids[block["tool_use_id"]] = cm.id

                missing = [
                    tc["id"]
                    for tc in pending_tool_calls
                    if tc["id"] not in placeholder_ids
                ]
                if missing:
                    logger.error(
                        f"OAuth resume: placeholder tool_result(s) missing for {missing} in chat {chat_id}"
                    )
                    yield f"event: error\ndata: Could not resume from OAuth — placeholder missing.\n\n"
                    return

                context = ToolContext(
                    chat_id=chat_id,
                    user_id=tool_user_id,
                    user_email=user_email,
                    user_configuration=user_configuration,
                    skip_permission_check=tool_skip_perm,
                )

                # Execute each pending tool call and collect the new blocks
                # keyed by tool_use_id for the in-place rewrite below.
                new_blocks_by_id: dict[str, ToolResultBlockParam] = {}
                for tc in pending_tool_calls:
                    result = await registry.execute(tc["name"], tc["input"], context)
                    new_blocks_by_id[tc["id"]] = ToolResultBlockParam(
                        type="tool_result",
                        tool_use_id=tc["id"],
                        content=result.content,
                        is_error=result.is_error,
                    )

                # Walk conversation_messages once. For every user message that
                # contains at least one matching placeholder, rewrite all
                # matches in that message and persist the JSONB update.
                touched_message_ids: set[str] = set()
                for cm_msg in conversation_messages:
                    if not isinstance(cm_msg, dict) or cm_msg.get("role") != "user":
                        continue
                    content = cm_msg.get("content")
                    if not isinstance(content, list):
                        continue
                    new_content = [
                        (
                            new_blocks_by_id[b["tool_use_id"]]
                            if (
                                isinstance(b, dict)
                                and b.get("type") == "tool_result"
                                and b.get("tool_use_id") in new_blocks_by_id
                            )
                            else b
                        )
                        for b in content
                    ]
                    if new_content == content:
                        continue
                    cm_msg["content"] = new_content
                    # All placeholders for the same assistant turn live in the
                    # same chat_messages row, so we resolve a single id from
                    # any matched tool_use_id.
                    matched_tu_id = next(
                        (
                            b["tool_use_id"]
                            for b in content
                            if isinstance(b, dict)
                            and b.get("type") == "tool_result"
                            and b.get("tool_use_id") in new_blocks_by_id
                        ),
                        None,
                    )
                    if matched_tu_id and matched_tu_id in placeholder_ids:
                        message_row_id = placeholder_ids[matched_tu_id]
                        if message_row_id not in touched_message_ids:
                            await messages_repo.update_message_content(
                                message_row_id, cm_msg
                            )
                            touched_message_ids.add(message_row_id)

                # Notify the client so it can swap the OAuth card(s) for the
                # real tool_results without waiting for a page refresh.
                for tu_id, new_block in new_blocks_by_id.items():
                    replaced_event = {
                        "tool_use_id": tu_id,
                        "content": new_block["content"],
                        "is_error": new_block["is_error"],
                    }
                    yield f"event: tool_result_replaced\ndata: {json.dumps(replaced_event)}\n\n"

            logger.info(
                f"Starting conversation with {len(conversation_messages)} initial messages"
            )

            # Extract the first user message query for caching purposes
            original_user_query = None
            for msg in conversation_messages:
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

            context = ToolContext(
                chat_id=chat_id,
                user_id=tool_user_id,
                user_email=user_email,
                user_configuration=user_configuration,
                original_user_query=original_user_query,
                skip_permission_check=tool_skip_perm,
            )

            usage_repo = UsageRepository()
            assistant_message: MessageParam | None = None

            for iteration in range(AGENT_MAX_ITERATIONS):
                # Stop only on an explicit user cancel (Stop button). The run is
                # decoupled from the client connection, so a backgrounded tab no
                # longer aborts generation.
                if await _is_run_cancelled(redis_client, chat_id):
                    logger.info(f"Run cancelled, stopping stream for chat {chat_id}")
                    break

                logger.info(f"Iteration {iteration + 1}/{AGENT_MAX_ITERATIONS}")
                content_blocks: list[TextBlockParam | ToolUseBlockParam] = []
                provider_extras = llm_provider.PERSISTED_BLOCK_EXTRAS

                # Rebuild the per-turn tool list. The set of loaded connector
                # loaded connector tools may have grown during the previous iteration via the
                # load_tool / load_tool_set meta-tools.
                turn_tools = build_turn_tools(
                    build_result.always_on_handlers,
                    build_result.connector_handler,
                    loaded_toolsets,
                )

                logger.info("Sending request to LLM provider")
                logger.debug(
                    f"Messages being sent: {json.dumps(conversation_messages, indent=2)}"
                )
                logger.debug(
                    f"Tools available: {[tool['name'] for tool in turn_tools]}"
                )

                tracker = UsageTracker(
                    usage_repo,
                    UsageContext(
                        user_id=chat.user_id,
                        model_id=llm_provider.model_record_id,
                        model_name=llm_provider.model_name,
                        provider_type=llm_provider.provider_type,
                        purpose=UsagePurpose.CHAT,
                        chat_id=chat_id,
                    ),
                )

                raw_stream: AsyncStream[MessageStreamEvent] = (
                    llm_provider.stream_response(
                        prompt="",  # Not used when messages provided
                        messages=conversation_messages,
                        tools=turn_tools,
                        max_tokens=DEFAULT_MAX_TOKENS,
                        temperature=DEFAULT_TEMPERATURE,
                        top_p=DEFAULT_TOP_P,
                        system_prompt=system_prompt,
                    )
                )

                stream = tracker.wrap_stream(raw_stream)

                event_index = 0
                message_stop_received = False
                cancelled = False
                last_cancel_check_at = 0.0
                async for event in stream:
                    logger.debug(f"Received event: {event} (index: {event_index})")
                    event_index += 1

                    now = asyncio.get_running_loop().time()
                    if now - last_cancel_check_at >= _CANCEL_CHECK_INTERVAL_SECONDS:
                        last_cancel_check_at = now
                        if await _is_run_cancelled(redis_client, chat_id):
                            cancelled = True
                            break

                    if event.type == "message_start":
                        logger.info("Message start received.")

                    if event.type == "content_block_delta":
                        logger.debug(
                            f"Content block delta received at index {event.index}: {event.delta}"
                        )
                        if event.delta.type == "text_delta":
                            if event.index >= len(content_blocks):
                                logger.warning(
                                    f"Received text delta for unknown content block index {event.index}, creating new text block"
                                )
                                content_blocks.append(
                                    TextBlockParam(type="text", text="")
                                )
                            text_block = cast(
                                TextBlockParam, content_blocks[event.index]
                            )
                            text_block["text"] += event.delta.text
                        elif event.delta.type == "input_json_delta":
                            if event.index >= len(content_blocks):
                                logger.warning(
                                    f"Received input JSON delta for unknown content block index {event.index}, creating new tool use block"
                                )
                                content_blocks.append(
                                    ToolUseBlockParam(
                                        type="tool_use", id="", name="", input=""
                                    )
                                )
                            tool_use_block = cast(
                                ToolUseBlockParam, content_blocks[event.index]
                            )
                            tool_use_block["input"] = (
                                cast(str, tool_use_block["input"])
                                + event.delta.partial_json
                            )
                        elif event.delta.type == "citations_delta":
                            if event.index >= len(content_blocks):
                                logger.warning(
                                    f"Received citations delta for unknown content block index {event.index}, creating new citations block"
                                )
                                content_blocks.append(
                                    TextBlockParam(type="text", text="", citations=[])
                                )
                            text_block = cast(
                                TextBlockParam, content_blocks[event.index]
                            )
                            if (
                                "citations" not in text_block
                                or not text_block["citations"]
                            ):
                                text_block["citations"] = []
                            citations = cast(
                                list[TextCitationParam], text_block["citations"]
                            )
                            citations.append(convert_citation_to_param(event.delta))
                    elif event.type == "content_block_start":
                        if event.content_block.type == "text":
                            logger.info(f"Text block start: {event.content_block.text}")
                            text_block: TextBlockParam = TextBlockParam(
                                type="text", text=event.content_block.text
                            )
                            _copy_provider_extras(
                                event.content_block, text_block, provider_extras
                            )
                            content_blocks.append(text_block)
                        elif event.content_block.type == "tool_use":
                            logger.info(
                                f"Tool use block start: {event.content_block.name} (id: {event.content_block.id})"
                            )
                            tool_block: ToolUseBlockParam = ToolUseBlockParam(
                                type="tool_use",
                                id=event.content_block.id,
                                name=event.content_block.name,
                                input="",
                            )
                            _copy_provider_extras(
                                event.content_block, tool_block, provider_extras
                            )
                            content_blocks.append(tool_block)
                    elif event.type == "citation":
                        logger.info(f"Citation received: {event.citation}")
                    elif event.type == "message_stop":
                        logger.info("Message stop received.")
                        message_stop_received = True

                    logger.debug(
                        f"Yielding event to client: {event.to_json(indent=None)}"
                    )
                    yield f"event: message\ndata: {event.to_json(indent=None)}\n\n"

                    if message_stop_received:
                        break

                if cancelled:
                    assistant_message = _partial_assistant_message(content_blocks)
                    if assistant_message is not None:
                        conversation_messages.append(assistant_message)
                        yield f"event: save_message\ndata: {json.dumps(assistant_message)}\n\n"
                    break

                tracker.save()

                # Parse tool call inputs. Convert to JSON.
                tool_calls = [b for b in content_blocks if b["type"] == "tool_use"]
                for tool_call in tool_calls:
                    try:
                        tool_call["input"] = json.loads(cast(str, tool_call["input"]))
                    except json.JSONDecodeError as e:
                        logger.error(
                            f"Failed to parse tool call input as JSON: {tool_call['input']}. Error: {e}"
                        )
                        tool_call["input"] = {}

                assistant_message = MessageParam(
                    role="assistant", content=content_blocks
                )
                conversation_messages.append(assistant_message)

                # Send complete message to omni-web for database persistence
                yield f"event: save_message\ndata: {json.dumps(assistant_message)}\n\n"

                # If no tool calls, we're done
                if not tool_calls:
                    logger.info(
                        f"No tool calls in iteration {iteration + 1}, completing response"
                    )
                    break

                logger.info(f"Processing {len(tool_calls)} tool calls")

                # Stop before expensive tool execution if the user cancelled.
                if await _is_run_cancelled(redis_client, chat_id):
                    logger.info(
                        f"Run cancelled before tool execution for chat {chat_id}"
                    )
                    break

                # Execute each tool call via the registry
                tool_results: list[ToolResultBlockParam] = []
                # Track every tool call in this iteration that surfaced an
                # oauth_required envelope so we can pause the agent loop after
                # the iteration's tool_results are persisted (every tool_use
                # must be paired with a tool_result before we can pause, per
                # the Anthropic API contract). The frontend dedupes the cards
                # by sourceId; on resume we re-execute *every* pending call
                # so all hidden placeholders also get replaced with real
                # results.
                loop_pending_oauths: list[tuple[dict, OAuthRequiredPayload]] = []
                for tool_call in tool_calls:
                    tool_name = tool_call["name"]

                    # Check if this tool requires approval
                    if registry.requires_approval(tool_name):
                        logger.info(
                            f"Tool {tool_name} requires approval, pausing stream"
                        )

                        # Save state to Redis for resume
                        if redis_client:
                            approval_id = await _save_pending_approval(
                                redis_client,
                                chat_id,
                                tool_call,
                                conversation_messages,
                            )

                            # Emit approval_required event
                            approval_event = {
                                "approval_id": approval_id,
                                "tool_name": tool_name,
                                "tool_input": tool_call["input"],
                                "tool_call_id": tool_call["id"],
                            }
                            yield f"event: approval_required\ndata: {json.dumps(approval_event)}\n\n"
                            yield "event: end_of_stream\ndata: Approval required\n\n"
                            return
                        else:
                            # No Redis, can't do approvals — treat as denied
                            tool_results.append(
                                ToolResultBlockParam(
                                    type="tool_result",
                                    tool_use_id=tool_call["id"],
                                    content=[
                                        {
                                            "type": "text",
                                            "text": "This action requires user approval, but the approval system is not available.",
                                        }
                                    ],
                                    is_error=True,
                                )
                            )
                            continue

                    # Execute the tool
                    result = await registry.execute(
                        tool_name, tool_call["input"], context
                    )

                    tool_result = ToolResultBlockParam(
                        type="tool_result",
                        tool_use_id=tool_call["id"],
                        content=result.content,
                        is_error=result.is_error,
                    )
                    tool_results.append(tool_result)

                    if result.oauth_required is not None:
                        loop_pending_oauths.append((tool_call, result.oauth_required))

                    yield f"event: message\ndata: {json.dumps(tool_result)}\n\n"

                tool_result_message = MessageParam(role="user", content=tool_results)
                conversation_messages.append(tool_result_message)

                # Send complete tool result message to omni-web for database persistence
                yield f"event: save_message\ndata: {json.dumps(tool_result_message)}\n\n"

                # If any tool surfaced an oauth_required envelope, pause the loop now
                # that the placeholder tool_result has been persisted. The frontend
                # will render the Connect card; on completion it re-opens the stream
                # and the resume branch (top of stream_generator) replaces the
                # placeholder with the real result.
                if loop_pending_oauths and redis_client:
                    pending_tcs = [tc for (tc, _) in loop_pending_oauths]
                    await _save_pending_oauth(
                        redis_client, chat_id, pending_tcs, conversation_messages
                    )
                    # Surface a single oauth_required event for the first
                    # pending call — the frontend already dedupes the cards
                    # by sourceId, so emitting more would be redundant.
                    primary_tc, primary_payload = loop_pending_oauths[0]
                    oauth_event = {
                        "tool_call_id": primary_tc["id"],
                        "tool_name": primary_tc["name"],
                        "source_id": primary_payload.source_id,
                        "source_type": primary_payload.source_type,
                        "provider": primary_payload.provider,
                        "oauth_start_url": primary_payload.oauth_start_url,
                    }
                    yield f"event: oauth_required\ndata: {json.dumps(oauth_event)}\n\n"
                    yield f"event: end_of_stream\ndata: OAuth required\n\n"
                    return

            # Memory write (fire-and-forget)
            if (
                memory_provider is not None
                and memory_write_key
                and effective_mode >= MemoryMode.CHAT
            ):
                try:
                    last_user_content = None
                    for msg in reversed(conversation_messages):
                        m = msg if isinstance(msg, dict) else dict(msg)
                        if m.get("role") == "user":
                            raw = m.get("content", "")
                            if isinstance(raw, list):
                                # Extract text blocks only — skip image/tool_result blocks
                                # so the provider never sees non-text content blocks.
                                raw = " ".join(
                                    b.get("text", "")
                                    for b in raw
                                    if isinstance(b, dict) and b.get("type") == "text"
                                )
                            if not raw:
                                # Tool-result messages have no text — keep scanning back.
                                continue
                            last_user_content = raw
                            break
                    if last_user_content and assistant_message:
                        assistant_content = "".join(
                            b.get("text", "")
                            for b in assistant_message.get("content", [])
                            if isinstance(b, dict) and b.get("type") == "text"
                        )
                        if assistant_content:
                            turn = [
                                MessageParam(role="user", content=last_user_content),
                                MessageParam(
                                    role="assistant", content=assistant_content
                                ),
                            ]
                            asyncio.create_task(
                                memory_provider.add(messages=turn, key=memory_write_key)
                            )
                except Exception as e:
                    logger.warning(f"Memory write setup failed for chat {chat_id}: {e}")

            yield "event: end_of_stream\ndata: Stream ended\n\n"

        except asyncio.CancelledError:
            logger.info(f"Stream cancelled for chat {chat_id}")
            raise  # Re-raise to let FastAPI handle cleanup
        except Exception as e:
            logger.error(
                f"Failed to generate AI response with tools: {e}", exc_info=True
            )
            yield _sse_event("stream_error", _chat_error_payload(e))

    # First new message persisted by the producer is normally the child of the
    # latest active-path row. Approval resumes are different: the user may have
    # reloaded or tried to branch while the approval card was hidden, so use the
    # assistant message that actually contains the pending tool_use as parent.
    if pending:
        parent_id = await _find_tool_use_message_id(
            messages_repo, chat_id, pending["tool_call"]["id"]
        )
        if parent_id is None:
            parent_id = chat_messages[-1].id if chat_messages else None
    else:
        parent_id = chat_messages[-1].id if chat_messages else None

    if redis_client is None:
        # No Redis: run inline (no resume), still persisting via the producer wrapper.
        return StreamingResponse(
            _persist_and_transform(
                stream_generator(), chat_id, messages_repo, parent_id
            ),
            media_type="text/event-stream",
            headers=SSE_HEADERS,
        )

    # Single producer per chat: the lock winner starts the background run; a
    # racing connect attaches to it instead of starting a duplicate.
    got_lock = await redis_client.set(
        _run_lock_key(chat_id), "1", nx=True, ex=_RUN_LOCK_TTL
    )
    if not got_lock:
        return StreamingResponse(
            _consume_run(redis_client, chat_id, last_event_id or "0"),
            media_type="text/event-stream",
            headers=SSE_HEADERS,
        )

    await redis_client.delete(_stream_key(chat_id))
    await redis_client.delete(_cancel_key(chat_id))
    task = asyncio.create_task(
        _run_producer(
            redis_client, chat_id, stream_generator(), messages_repo, parent_id
        )
    )
    _background_run_tasks.add(task)
    task.add_done_callback(_background_run_tasks.discard)

    return StreamingResponse(
        _consume_run(redis_client, chat_id, "0"),
        media_type="text/event-stream",
        headers=SSE_HEADERS,
    )


@router.post("/chat/{chat_id}/cancel")
async def cancel_chat_stream(
    request: Request, chat_id: str = Path(..., description="Chat thread ID")
):
    """Explicit Stop: signal the background run to stop at its next checkpoint."""
    redis_client = getattr(request.app.state, "redis_client", None)
    if redis_client is not None:
        try:
            await redis_client.set(_cancel_key(chat_id), "1", ex=_CANCEL_TTL)
        except Exception as e:
            logger.error(f"Failed to set cancel flag for chat {chat_id}: {e}")
    return {"status": "cancelling"}


@router.post("/chat/{chat_id}/generate_title")
async def generate_chat_title(
    request: Request, chat_id: str = Path(..., description="Chat thread ID")
):
    """Generate a title for a chat thread based on its first messages"""
    logger.info(f"Generating title for chat: {chat_id}")

    try:
        # Get chat from database
        chats_repo = ChatsRepository()
        chat = await chats_repo.get(chat_id)
        if not chat:
            raise HTTPException(status_code=404, detail="Chat thread not found")

        llm_provider = _resolve_secondary_provider(request.app.state)

        # Check if title already exists
        if chat.title:
            logger.info(f"Chat already has a title: {chat.title}")
            return {"title": chat.title, "status": "existing"}

        # Get messages from database
        messages_repo = MessagesRepository()
        chat_messages = await messages_repo.get_by_chat(chat_id)
        if not chat_messages:
            raise HTTPException(
                status_code=400, detail="Not enough messages to generate title"
            )

        # Use only the user's first text-bearing message to generate the title.
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
        logger.debug(f"Conversation text: {conversation_text[:200]}...")

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

        # Update chat with the new title
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


@router.get("/chat/{chat_id}/artifacts/{path:path}")
async def download_artifact(
    request: Request,
    chat_id: str = Path(..., description="Chat thread ID"),
    path: str = Path(..., description="Relative file path in the sandbox"),
):
    """Proxy artifact downloads from the sandbox service."""
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
