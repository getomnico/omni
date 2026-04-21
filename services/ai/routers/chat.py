import asyncio
import json
import logging
import pathlib
import re
from dataclasses import dataclass

import httpx
from fastapi import APIRouter, HTTPException, Path, Query, Request
from fastapi.responses import Response, StreamingResponse
from pydantic import ValidationError

from agent_loop import (
    AssistantMessageComplete,
    LLMStreamEvent,
    LoopComplete,
    ToolApprovalRequired,
    ToolResultEvent,
    ToolResultMessageComplete,
    run_agent_loop,
)
from agents.executor import _build_source_filter
from agents.models import Agent
from agents.repository import AgentRepository, AgentRunRepository
from attachments import expand_uploads
from db import ChatsRepository, MessagesRepository
from db.documents import DocumentsRepository
from db.uploads import UploadsRepository
from db.models import Chat, Source
from db.users import UsersRepository
from tools import (
    SearcherTool,
    ToolRegistry,
    ToolContext,
    SearchToolHandler,
    ConnectorToolHandler,
    DocumentToolHandler,
    PeopleSearchHandler,
)
from tools.connector_handler import ConnectorAction, SearchOperator
from tools.sandbox_handler import SandboxToolHandler
from tools.search_handler import fetch_operator_values
from tools.skill_handler import SkillHandler
from config import (
    DEFAULT_MAX_TOKENS,
    DEFAULT_TEMPERATURE,
    DEFAULT_TOP_P,
    AGENT_MAX_ITERATIONS,
    CONNECTOR_MANAGER_URL,
    APPROVAL_TIMEOUT_SECONDS,
    SANDBOX_URL,
)
from db.usage import UsageRepository
from providers import LLMProvider
from prompts import build_chat_system_prompt, build_agent_chat_system_prompt
from services.compaction import ConversationCompactor
from services.usage import UsageTracker, UsageContext, UsagePurpose, track_usage
from state import AppState

from anthropic.types import (
    MessageParam,
    TextCitationParam,
    CitationCharLocationParam,
    CitationPageLocationParam,
    CitationContentBlockLocationParam,
    CitationSearchResultLocationParam,
    CitationWebSearchResultLocationParam,
    CitationsDelta,
    ToolResultBlockParam,
    SearchResultBlockParam,
    CitationsConfigParam,
)


router = APIRouter(tags=["chat"])
logger = logging.getLogger(__name__)

TITLE_GENERATION_SYSTEM_PROMPT = """You are a helpful assistant that generates concise, descriptive titles for chat conversations.
Based on the first message(s) of a conversation, generate a title that is:
- 3-7 words long
- Descriptive and specific
- Written in title case
- Does not include quotes or special formatting

Just respond with the title text, nothing else."""


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
    connector_actions: list[ConnectorAction] | None
    sources: list[Source] | None
    search_operators: list[SearchOperator] | None


async def _fetch_sources_from_connector_manager() -> list[Source] | None:
    """Fetch all sources from the connector manager. Returns None on failure."""
    if not CONNECTOR_MANAGER_URL:
        return None
    try:
        async with httpx.AsyncClient(timeout=10.0) as client:
            resp = await client.get(f"{CONNECTOR_MANAGER_URL.rstrip('/')}/sources")
            resp.raise_for_status()
            return [Source.from_row(s) for s in resp.json()]
    except Exception as e:
        logger.warning(f"Failed to fetch sources from connector manager: {e}")
        return None


async def _build_registry(request: Request, chat: Chat) -> RegistryResult:
    """Build a ToolRegistry with all available handlers."""
    registry = ToolRegistry()

    # Fetch sources from connector manager once, share with all handlers
    sources = await _fetch_sources_from_connector_manager()

    connector_actions: list[ConnectorAction] | None = None
    search_operators: list[SearchOperator] | None = None

    # Register connector tools if connector-manager is configured
    if CONNECTOR_MANAGER_URL:
        connector_handler = ConnectorToolHandler(
            connector_manager_url=CONNECTOR_MANAGER_URL,
            user_id=chat.user_id,
            redis_client=getattr(request.app.state, "redis_client", None),
            prefetched_sources=sources,
            documents_repo=DocumentsRepository(),
            sandbox_url=SANDBOX_URL,
        )
        await connector_handler._ensure_initialized()
        registry.register(connector_handler)

        # Collect action metadata for system prompt
        if connector_handler._actions:
            connector_actions = list(connector_handler._actions.values())

        # Collect search operators for search tool description
        if connector_handler.search_operators:
            search_operators = connector_handler.search_operators

    # Fetch dynamic operator values for enriched search tool description
    active_sources = [s for s in (sources or []) if s.is_active and not s.is_deleted]
    connected_source_types = list({s.source_type for s in active_sources})
    operator_values: dict[str, list[str]] = {}
    if search_operators:
        operator_values = await fetch_operator_values(
            request.app.state.searcher_tool.client,
            search_operators,
            redis_client=getattr(request.app.state, "redis_client", None),
        )

    # Register search tools (with dynamic operators from connector manifests)
    registry.register(
        SearchToolHandler(
            searcher_tool=request.app.state.searcher_tool,
            search_operators=search_operators,
            connected_source_types=connected_source_types,
            operator_values=operator_values,
        )
    )

    # Register people search tool
    registry.register(
        PeopleSearchHandler(searcher_tool=request.app.state.searcher_tool)
    )

    # Register document handler (unified read_document tool)
    content_storage = getattr(request.app.state, "content_storage", None)
    if content_storage or CONNECTOR_MANAGER_URL:
        registry.register(
            DocumentToolHandler(
                content_storage=content_storage,
                documents_repo=DocumentsRepository(),
                sandbox_url=SANDBOX_URL,
                connector_manager_url=CONNECTOR_MANAGER_URL or None,
            )
        )

    # Register sandbox tools if sandbox service is configured
    if SANDBOX_URL:
        registry.register(SandboxToolHandler(sandbox_url=SANDBOX_URL))

    # Register skill loader (load_skill tool)
    skills_dir = pathlib.Path(__file__).resolve().parent.parent / "skills"
    skill_handler = SkillHandler(skills_dir=skills_dir)
    if skill_handler._available:
        registry.register(skill_handler)

    return RegistryResult(
        registry=registry,
        connector_actions=connector_actions,
        sources=sources,
        search_operators=search_operators,
    )


async def _build_agent_chat_registry(request: Request, agent: Agent) -> RegistryResult:
    """Build a read-only ToolRegistry for agent chat sessions.

    Uses the agent's own permissions (matching the background executor):
    org agents read across everything; user agents are scoped by allowed_sources.
    Write/connector-action tools are intentionally not registered — agent chats are read-only.
    """
    registry = ToolRegistry()

    sources = await _fetch_sources_from_connector_manager()

    source_filter = _build_source_filter(agent) if agent.agent_type == "user" else None

    # We still need connector handler for search operators, but won't register it
    search_operators = None
    if CONNECTOR_MANAGER_URL:
        connector_handler = ConnectorToolHandler(
            connector_manager_url=CONNECTOR_MANAGER_URL,
            user_id=agent.user_id if agent.agent_type == "user" else "",
            redis_client=getattr(request.app.state, "redis_client", None),
            prefetched_sources=sources,
            source_filter=source_filter,
            documents_repo=DocumentsRepository(),
        )
        await connector_handler._ensure_initialized()
        if connector_handler.search_operators:
            search_operators = connector_handler.search_operators

    active_sources = [s for s in (sources or []) if s.is_active and not s.is_deleted]
    connected_source_types = list({s.source_type for s in active_sources})
    operator_values: dict[str, list[str]] = {}
    if search_operators:
        operator_values = await fetch_operator_values(
            request.app.state.searcher_tool.client,
            search_operators,
            redis_client=getattr(request.app.state, "redis_client", None),
        )

    registry.register(
        SearchToolHandler(
            searcher_tool=request.app.state.searcher_tool,
            search_operators=search_operators,
            connected_source_types=connected_source_types,
            operator_values=operator_values,
        )
    )

    registry.register(
        PeopleSearchHandler(searcher_tool=request.app.state.searcher_tool)
    )

    content_storage = getattr(request.app.state, "content_storage", None)
    if content_storage or CONNECTOR_MANAGER_URL:
        registry.register(
            DocumentToolHandler(
                content_storage=content_storage,
                documents_repo=DocumentsRepository(),
                sandbox_url=SANDBOX_URL,
                connector_manager_url=CONNECTOR_MANAGER_URL or None,
            )
        )

    if SANDBOX_URL:
        registry.register(SandboxToolHandler(sandbox_url=SANDBOX_URL))

    skills_dir = pathlib.Path(__file__).resolve().parent.parent / "skills"
    skill_handler = SkillHandler(skills_dir=skills_dir)
    if skill_handler._available:
        registry.register(skill_handler)

    return RegistryResult(
        registry=registry,
        connector_actions=None,
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
    messages_repo = MessagesRepository()
    chat_messages = await messages_repo.get_active_path(chat_id)

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

        # Handle auto_start: inject ephemeral message when no messages exist
        if not chat_messages:
            if auto_start:
                chat_messages = []
            else:
                raise HTTPException(
                    status_code=404, detail="No messages found for chat"
                )

        build_result = await _build_agent_chat_registry(request, agent)
        registry = build_result.registry
        all_tools = registry.get_all_tools()
        pending = None  # no approval flow for agent chats

        # Build agent chat system prompt with run history
        run_repo = AgentRunRepository()
        runs = await run_repo.list_runs(agent.id, limit=20)
        active_sources = [
            s for s in (build_result.sources or []) if s.is_active and not s.is_deleted
        ]
        system_prompt = build_agent_chat_system_prompt(
            agent,
            runs,
            active_sources,
            user_name=user_name,
            user_email=user_email,
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
        if chat.user_id:
            users_repo = UsersRepository()
            user = await users_repo.find_by_id(chat.user_id)
            if user:
                user_email = user.email
                user_name = user.full_name

        if not chat_messages:
            raise HTTPException(status_code=404, detail="No messages found for chat")

        build_result = await _build_registry(request, chat)
        registry = build_result.registry
        all_tools = registry.get_all_tools()

        # Check for pending approval resume flow
        pending = None
        if redis_client:
            pending = await _get_pending_approval(redis_client, chat_id)

        active_sources = [
            s for s in (build_result.sources or []) if s.is_active and not s.is_deleted
        ]
        system_prompt = build_chat_system_prompt(
            active_sources,
            build_result.connector_actions,
            user_name=user_name,
            user_email=user_email,
        )

        messages: list[MessageParam] = [
            MessageParam(**msg.message) for msg in chat_messages
        ]

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

    # Check if we need to process - only if last message is from user (or resuming from approval)
    last_message_role = messages[-1].get("role") if messages else None
    if not pending and last_message_role != "user":
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
    if compactor.needs_compaction(messages, all_tools):
        logger.info(f"Compacting conversation for chat {chat_id}")
        messages = await compactor.compact_conversation(chat_id, messages)

    # Stream AI response with tool calling
    async def stream_generator():
        try:
            conversation_messages = messages.copy()

            # Handle approval resume
            if pending:
                logger.info(f"Resuming from pending approval for chat {chat_id}")
                await _clear_pending_approval(redis_client, chat_id)

                tool_call = pending["tool_call"]

                # Check if this was approved or denied by looking at DB
                # For now, if we're resuming, it was approved (the frontend only
                # re-invokes the stream after approval)
                context = ToolContext(
                    chat_id=chat_id,
                    user_id=tool_user_id,
                    user_email=user_email,
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

                tool_result_message = MessageParam(role="user", content=[tool_result])
                conversation_messages.append(tool_result_message)
                yield f"event: save_message\ndata: {json.dumps(tool_result_message)}\n\n"

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
                original_user_query=original_user_query,
                skip_permission_check=tool_skip_perm,
            )

            usage_repo = UsageRepository()

            current_tracker: UsageTracker | None = None

            def wrap_stream(raw_stream):
                nonlocal current_tracker
                current_tracker = UsageTracker(
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
                return current_tracker.wrap_stream(raw_stream)

            async def on_iteration_end():
                if current_tracker:
                    current_tracker.save()

            async def should_stop() -> bool:
                disconnected = await request.is_disconnected()
                if disconnected:
                    logger.info(f"Client disconnected, stopping stream for chat {chat_id}")
                return disconnected

            async def approval_handler(_tool_call) -> str:
                # Without Redis we can't persist approval state, so fall back to
                # the legacy behavior of emitting a soft-denied tool_result and
                # continuing the loop instead of aborting the whole stream.
                return "pause" if redis_client else "deny"

            async for ev in run_agent_loop(
                llm_provider=llm_provider,
                messages=conversation_messages,
                system_prompt=system_prompt,
                tools=all_tools,
                registry=registry,
                tool_context=context,
                max_iterations=AGENT_MAX_ITERATIONS,
                max_tokens=DEFAULT_MAX_TOKENS,
                temperature=DEFAULT_TEMPERATURE,
                top_p=DEFAULT_TOP_P,
                wrap_stream=wrap_stream,
                should_stop=should_stop,
                on_iteration_end=on_iteration_end,
                approval_handler=approval_handler,
            ):
                if isinstance(ev, LLMStreamEvent):
                    yield f"event: message\ndata: {ev.raw_event.to_json(indent=None)}\n\n"
                elif isinstance(ev, AssistantMessageComplete):
                    yield f"event: save_message\ndata: {json.dumps(ev.message)}\n\n"
                elif isinstance(ev, ToolResultEvent):
                    yield f"event: message\ndata: {json.dumps(ev.tool_result)}\n\n"
                elif isinstance(ev, ToolResultMessageComplete):
                    yield f"event: save_message\ndata: {json.dumps(ev.message)}\n\n"
                elif isinstance(ev, ToolApprovalRequired):
                    approval_id = await _save_pending_approval(
                        redis_client,
                        chat_id,
                        ev.tool_call,
                        conversation_messages,
                    )
                    approval_event = {
                        "approval_id": approval_id,
                        "tool_name": ev.tool_call["name"],
                        "tool_input": ev.tool_call["input"],
                        "tool_call_id": ev.tool_call["id"],
                    }
                    yield f"event: approval_required\ndata: {json.dumps(approval_event)}\n\n"
                    yield f"event: end_of_stream\ndata: Approval required\n\n"
                    return
                elif isinstance(ev, LoopComplete):
                    pass

            yield f"event: end_of_stream\ndata: Stream ended\n\n"

        except asyncio.CancelledError:
            logger.info(f"Stream cancelled for chat {chat_id}")
            raise  # Re-raise to let FastAPI handle cleanup
        except Exception as e:
            logger.error(
                f"Failed to generate AI response with tools: {e}", exc_info=True
            )
            yield f"event: error\ndata: Something went wrong, please try again later.\n\n"

    return StreamingResponse(
        stream_generator(),
        media_type="text/event-stream",
        headers={"Cache-Control": "no-cache", "Connection": "keep-alive"},
    )


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

        # Use only the user's first message to generate the title
        conversation_text = ""
        for msg in chat_messages:
            role = msg.message.get("role", "unknown")
            if role == "user":
                content = msg.message.get("content", "")
                if isinstance(content, str):
                    conversation_text += f"User: {content}\n"
                    break

        if not conversation_text.strip():
            raise HTTPException(
                status_code=400, detail="Could not extract conversation content"
            )

        logger.info(f"Extracted conversation text ({len(conversation_text)} chars)")
        logger.debug(f"Conversation text: {conversation_text[:200]}...")

        # Generate title using LLM
        prompt = f"{TITLE_GENERATION_SYSTEM_PROMPT}\n\nConversation:\n{conversation_text}\n\nTitle:"

        generated_title, title_usage = await llm_provider.generate_response(
            prompt=prompt,
            max_tokens=20,
            temperature=0.7,
            top_p=0.9,
        )

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
            input_tokens=title_usage.input_tokens,
            output_tokens=title_usage.output_tokens,
            cache_read_tokens=title_usage.cache_read_tokens,
            cache_creation_tokens=title_usage.cache_creation_tokens,
        )

        # Clean up the title
        title = generated_title.strip().strip('"').strip("'")

        # Limit title length just in case
        if len(title) > 100:
            title = title[:97] + "..."

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
