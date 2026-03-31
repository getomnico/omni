import asyncio
import json
import logging
import re
from dataclasses import dataclass, field
from typing import cast

import httpx
from fastapi import APIRouter, HTTPException, Path, Request
from fastapi.responses import Response, StreamingResponse
from pydantic import ValidationError

from db import ChatsRepository, MessagesRepository
from db.documents import DocumentsRepository
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
from config import (
    DEFAULT_MAX_TOKENS,
    DEFAULT_TEMPERATURE,
    DEFAULT_TOP_P,
    AGENT_MAX_ITERATIONS,
    CONNECTOR_MANAGER_URL,
    APPROVAL_TIMEOUT_SECONDS,
    SANDBOX_URL,
)
from providers import LLMProvider
from prompts import build_chat_system_prompt
from services.compaction import ConversationCompactor
from state import AppState

from anthropic import MessageStreamEvent, AsyncStream
from anthropic.types import (
    MessageParam,
    TextBlockParam,
    ToolUseBlockParam,
    TextCitationParam,
    CitationCharLocationParam,
    CitationPageLocationParam,
    CitationContentBlockLocationParam,
    CitationSearchResultLocationParam,
    CitationWebSearchResultLocationParam,
    CitationsDelta,
    CitationsSearchResultLocation,
    CitationCharLocation,
    RawContentBlockDeltaEvent,
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


def _resolve_llm_provider(state: AppState, chat: Chat) -> LLMProvider:
    """Resolve which LLM provider to use for a chat.
    Priority: chat's model -> default model -> first available.
    """
    models = state.models
    if not models:
        raise HTTPException(status_code=503, detail="No models configured")

    if chat.model_id and chat.model_id in models:
        return models[chat.model_id]

    if state.default_model_id and state.default_model_id in models:
        return models[state.default_model_id]

    return next(iter(models.values()))


def _resolve_secondary_provider(state: AppState) -> LLMProvider:
    """Resolve the secondary (lightweight) model provider.
    Priority: secondary model -> default model -> first available.
    Used for title generation, suggested questions, compaction, etc.
    """
    models = state.models
    if not models:
        raise HTTPException(status_code=503, detail="No models configured")

    if state.secondary_model_id and state.secondary_model_id in models:
        return models[state.secondary_model_id]

    if state.default_model_id and state.default_model_id in models:
        return models[state.default_model_id]

    return next(iter(models.values()))


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


# ---------------------------------------------------------------------------
# Synthetic citations for non-Anthropic providers
# ---------------------------------------------------------------------------

_CITATION_PATTERN = re.compile(r"\[citation:(\d{1,3})\]")

CITATION_INSTRUCTION = (
    "\n\n# Citing sources\n"
    "When referencing information from search results or documents, you MUST cite the source "
    "using the exact format [citation:n] where n is the source number. For example: "
    '"The quarterly revenue increased by 15% [citation:1] while expenses decreased [citation:3]." '
    "Always place the citation immediately after the relevant claim."
)


@dataclass
class CitableRef:
    """Tracks a citable content block (search result or document) for synthetic citation generation."""

    index: int
    title: str
    source: str
    cited_text: str
    ref_type: str  # "search_result" or "document"


def _build_citable_index(
    messages: list[MessageParam],
) -> dict[int, CitableRef]:
    """Scan all messages for search_result and document blocks in tool results.

    Returns a 1-based index mapping to CitableRef metadata.
    """
    index_map: dict[int, CitableRef] = {}
    counter = 1
    for msg in messages:
        content = msg.get("content", [])
        if not isinstance(content, list):
            continue
        for block in content:
            if not isinstance(block, dict) or block.get("type") != "tool_result":
                continue
            for sub_block in block.get("content", []):
                if not isinstance(sub_block, dict):
                    continue
                block_type = sub_block.get("type")

                if block_type == "search_result":
                    inner_texts = [
                        ib.get("text", "")
                        for ib in sub_block.get("content", [])
                        if isinstance(ib, dict) and ib.get("type") == "text"
                    ]
                    index_map[counter] = CitableRef(
                        index=counter,
                        title=sub_block.get("title", ""),
                        source=sub_block.get("source", ""),
                        cited_text="\n".join(inner_texts)[:500],
                        ref_type="search_result",
                    )
                    counter += 1

                elif block_type == "document":
                    source_obj = sub_block.get("source", {})
                    data = (
                        source_obj.get("data", "")
                        if isinstance(source_obj, dict)
                        else ""
                    )
                    index_map[counter] = CitableRef(
                        index=counter,
                        title=sub_block.get("title", ""),
                        source=sub_block.get("title", ""),
                        cited_text=data[:500],
                        ref_type="document",
                    )
                    counter += 1

    return index_map


def _prepare_messages_for_non_citation_provider(
    messages: list[MessageParam],
    citable_index: dict[int, CitableRef],
) -> list[MessageParam]:
    """Create a copy of messages with search_result/document blocks replaced by numbered text.

    Does not mutate the original messages.
    """
    if not citable_index:
        return messages

    transformed: list[MessageParam] = []
    counter = 1
    for msg in messages:
        content = msg.get("content", [])
        if not isinstance(content, list):
            transformed.append(msg)
            continue

        new_content = []
        has_changes = False
        for block in content:
            if not isinstance(block, dict) or block.get("type") != "tool_result":
                new_content.append(block)
                continue

            sub_blocks = block.get("content", [])
            has_citable = any(
                isinstance(sb, dict) and sb.get("type") in ("search_result", "document")
                for sb in sub_blocks
            )

            if not has_citable:
                new_content.append(block)
                continue

            has_changes = True
            new_sub_blocks = []
            for sb in sub_blocks:
                if not isinstance(sb, dict):
                    new_sub_blocks.append(sb)
                    continue

                sb_type = sb.get("type")
                if sb_type == "search_result":
                    ref = citable_index.get(counter)
                    if ref:
                        inner_texts = [
                            ib.get("text", "")
                            for ib in sb.get("content", [])
                            if isinstance(ib, dict) and ib.get("type") == "text"
                        ]
                        new_sub_blocks.append(
                            {
                                "type": "text",
                                "text": f"[{counter}] {ref.title} ({ref.source})\n"
                                + "\n".join(inner_texts),
                            }
                        )
                    counter += 1

                elif sb_type == "document":
                    ref = citable_index.get(counter)
                    if ref:
                        source_obj = sb.get("source", {})
                        data = (
                            source_obj.get("data", "")
                            if isinstance(source_obj, dict)
                            else ""
                        )
                        new_sub_blocks.append(
                            {
                                "type": "text",
                                "text": f"[{counter}] Document: {ref.title}\n{data}",
                            }
                        )
                    counter += 1

                else:
                    new_sub_blocks.append(sb)

            new_block = {**block, "content": new_sub_blocks}
            new_content.append(new_block)

        if has_changes:
            transformed.append({**msg, "content": new_content})
        else:
            transformed.append(msg)

    return transformed


def _extract_synthetic_citations(
    text: str,
    citable_index: dict[int, CitableRef],
) -> tuple[str, list[TextCitationParam]]:
    """Extract [citation:n] references from text and create synthetic citation objects.

    Returns (cleaned_text, citations) where cleaned_text has markers stripped.
    """
    citations: list[TextCitationParam] = []
    seen: set[int] = set()

    for match in _CITATION_PATTERN.finditer(text):
        ref_num = int(match.group(1))
        if ref_num in seen:
            continue
        seen.add(ref_num)

        ref = citable_index.get(ref_num)
        if ref is None:
            continue

        if ref.ref_type == "search_result":
            citations.append(
                CitationSearchResultLocationParam(
                    type="search_result_location",
                    start_block_index=0,
                    end_block_index=0,
                    search_result_index=ref.index - 1,  # 0-based
                    title=ref.title,
                    source=ref.source,
                    cited_text=ref.cited_text[:200],
                )
            )
        elif ref.ref_type == "document":
            citations.append(
                CitationCharLocationParam(
                    type="char_location",
                    document_index=ref.index - 1,  # 0-based
                    document_title=ref.title,
                    start_char_index=0,
                    end_char_index=0,
                    cited_text=ref.cited_text[:200],
                )
            )

    cleaned_text = _CITATION_PATTERN.sub("", text) if citations else text
    return cleaned_text, citations


def _build_synthetic_citation_event(
    block_idx: int,
    citation_param: TextCitationParam,
) -> RawContentBlockDeltaEvent:
    """Build an Anthropic-compatible citation delta event from a synthetic citation."""
    param = cast(dict, citation_param)
    if param["type"] == "search_result_location":
        citation_obj = CitationsSearchResultLocation(
            type="search_result_location",
            search_result_index=param["search_result_index"],
            start_block_index=param["start_block_index"],
            end_block_index=param["end_block_index"],
            title=param["title"],
            source=param["source"],
            cited_text=param["cited_text"],
        )
    else:
        citation_obj = CitationCharLocation(
            type="char_location",
            document_index=param["document_index"],
            document_title=param.get("document_title"),
            start_char_index=param["start_char_index"],
            end_char_index=param["end_char_index"],
            cited_text=param["cited_text"],
        )

    return RawContentBlockDeltaEvent(
        type="content_block_delta",
        index=block_idx,
        delta=CitationsDelta(type="citations_delta", citation=citation_obj),
    )


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
                sandbox_url=SANDBOX_URL or None,
                connector_manager_url=CONNECTOR_MANAGER_URL or None,
            )
        )

    # Register sandbox tools if sandbox service is configured
    if SANDBOX_URL:
        registry.register(SandboxToolHandler(sandbox_url=SANDBOX_URL))

    return RegistryResult(
        registry=registry,
        connector_actions=connector_actions,
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
    request: Request, chat_id: str = Path(..., description="Chat thread ID")
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

    # Resolve user info for permission checks and system prompt
    user_email: str | None = None
    user_name: str | None = None
    if chat.user_id:
        users_repo = UsersRepository()
        user = await users_repo.find_by_id(chat.user_id)
        if user:
            user_email = user.email
            user_name = user.full_name

    messages_repo = MessagesRepository()
    chat_messages = await messages_repo.get_active_path(chat_id)
    if not chat_messages:
        raise HTTPException(status_code=404, detail="No messages found for chat")

    # Build registry and discover connector actions
    build_result = await _build_registry(request, chat)
    registry = build_result.registry
    all_tools = registry.get_all_tools()

    # Check for pending approval resume flow
    redis_client = getattr(request.app.state, "redis_client", None)
    pending = None
    if redis_client:
        pending = await _get_pending_approval(redis_client, chat_id)

    # Check if we need to process - only if last message is from user (or resuming from approval)
    last_message = chat_messages[-1]
    if not pending and last_message.message.get("role") != "user":
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

    # Build messages for conversation from stored messages
    messages: list[MessageParam] = [
        MessageParam(**msg.message) for msg in chat_messages
    ]

    # Check if conversation needs compaction — use secondary model for summarization
    secondary_provider = _resolve_secondary_provider(request.app.state)
    compactor = ConversationCompactor(
        llm_provider=secondary_provider,
        redis_client=redis_client,
    )
    if compactor.needs_compaction(messages, all_tools):
        logger.info(f"Compacting conversation for chat {chat_id}")
        messages = await compactor.compact_conversation(chat_id, messages)

    # Build system prompt from active sources
    active_sources = [
        s for s in (build_result.sources or []) if s.is_active and not s.is_deleted
    ]
    system_prompt = build_chat_system_prompt(
        active_sources,
        build_result.connector_actions,
        user_name=user_name,
        user_email=user_email,
    )

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
                    user_id=chat.user_id,
                    user_email=user_email,
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
                user_id=chat.user_id,
                user_email=user_email,
                original_user_query=original_user_query,
            )

            for iteration in range(AGENT_MAX_ITERATIONS):
                # Check if client disconnected before starting expensive operations
                if await request.is_disconnected():
                    logger.info(
                        f"Client disconnected, stopping stream for chat {chat_id}"
                    )
                    break

                logger.info(f"Iteration {iteration + 1}/{AGENT_MAX_ITERATIONS}")
                content_blocks: list[TextBlockParam | ToolUseBlockParam] = []

                # For non-citation providers, number search results/documents
                # and build an index for synthetic citation extraction
                citable_index: dict[int, CitableRef] = {}
                if llm_provider.supports_citations:
                    provider_messages = conversation_messages
                    provider_system_prompt = system_prompt
                else:
                    citable_index = _build_citable_index(conversation_messages)
                    provider_messages = _prepare_messages_for_non_citation_provider(
                        conversation_messages, citable_index
                    )
                    provider_system_prompt = (
                        (system_prompt or "") + CITATION_INSTRUCTION
                        if citable_index
                        else system_prompt
                    )

                logger.info(f"Sending request to LLM provider")
                logger.debug(
                    f"Messages being sent: {json.dumps(provider_messages, indent=2)}"
                )
                logger.debug(f"Tools available: {[tool['name'] for tool in all_tools]}")

                stream: AsyncStream[MessageStreamEvent] = llm_provider.stream_response(
                    prompt="",  # Not used when messages provided
                    messages=provider_messages,
                    tools=all_tools,
                    max_tokens=DEFAULT_MAX_TOKENS,
                    temperature=DEFAULT_TEMPERATURE,
                    top_p=DEFAULT_TOP_P,
                    system_prompt=provider_system_prompt,
                )

                event_index = 0
                message_stop_received = False
                async for event in stream:
                    logger.debug(f"Received event: {event} (index: {event_index})")
                    event_index += 1

                    if event.type == "message_start":
                        logger.info(f"Message start received.")

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
                            content_blocks.append(
                                TextBlockParam(
                                    type="text", text=event.content_block.text
                                )
                            )
                        elif event.content_block.type == "tool_use":
                            logger.info(
                                f"Tool use block start: {event.content_block.name} (id: {event.content_block.id})"
                            )
                            content_blocks.append(
                                ToolUseBlockParam(
                                    type="tool_use",
                                    id=event.content_block.id,
                                    name=event.content_block.name,
                                    input="",
                                )
                            )
                    elif event.type == "citation":
                        logger.info(f"Citation received: {event.citation}")
                    elif event.type == "message_stop":
                        logger.info(f"Message stop received.")
                        message_stop_received = True

                    logger.debug(
                        f"Yielding event to client: {event.to_json(indent=None)}"
                    )
                    yield f"event: message\ndata: {event.to_json(indent=None)}\n\n"

                    if message_stop_received:
                        break

                # Synthesize citations for non-citation providers
                if not llm_provider.supports_citations and citable_index:
                    for block_idx, block in enumerate(content_blocks):
                        if block.get("type") != "text":
                            continue
                        cleaned_text, synthetic_citations = (
                            _extract_synthetic_citations(
                                block.get("text", ""), citable_index
                            )
                        )
                        if synthetic_citations:
                            block["text"] = cleaned_text
                            block["citations"] = synthetic_citations
                            for cit in synthetic_citations:
                                synth_event = _build_synthetic_citation_event(
                                    block_idx, cit
                                )
                                yield f"event: message\ndata: {synth_event.to_json(indent=None)}\n\n"

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

                # Check for disconnection before expensive tool execution
                if await request.is_disconnected():
                    logger.info(
                        f"Client disconnected before tool execution, stopping stream for chat {chat_id}"
                    )
                    break

                # Execute each tool call via the registry
                tool_results: list[ToolResultBlockParam] = []
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
                            yield f"event: end_of_stream\ndata: Approval required\n\n"
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

                    yield f"event: message\ndata: {json.dumps(tool_result)}\n\n"

                tool_result_message = MessageParam(role="user", content=tool_results)
                conversation_messages.append(tool_result_message)

                # Send complete tool result message to omni-web for database persistence
                yield f"event: save_message\ndata: {json.dumps(tool_result_message)}\n\n"

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

        generated_title = await llm_provider.generate_response(
            prompt=prompt,
            max_tokens=20,
            temperature=0.7,
            top_p=0.9,
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
