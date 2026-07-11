"""Agent loop generator for the chat streaming pipeline.

Owns ``stream_generator``, the per-iteration event handler, and the message-
content helpers that the agent loop uses to inspect/repair conversation history.
"""

from __future__ import annotations

import asyncio
import json
import logging
from collections.abc import AsyncIterator, Iterable
from typing import Any, cast

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

from config import (
    AGENT_MAX_ITERATIONS,
    DEFAULT_MAX_TOKENS,
    DEFAULT_TEMPERATURE,
    DEFAULT_TOP_P,
)
from db.tool_approvals import ToolApproval, ToolApprovalStatus, ToolApprovalType
from db.usage import UsageRepository
from memory import MemoryMode
from providers import LLMProvider, ProviderError
from services.compaction import ConversationCompactor
from services.usage import UsageContext, UsagePurpose, UsageTracker
from streaming.persist import (
    EndOfStreamReason,
    approval_required_event,
    end_of_stream,
    oauth_event,
    parse_tool_call_inputs,
    partial_assistant_message,
    sse_event,
    stream_error_sse,
)
from streaming.run import _CANCEL_CHECK_INTERVAL_SECONDS, is_run_cancelled
from tools import (
    ConnectorToolHandler,
    ToolContext,
    ToolHandler,
    ToolRegistry,
)
from tools.omni_tool_result import OAuthRequiredPayload
from tools.turn_builder import build_turn_tools

logger = logging.getLogger(__name__)


# ---------------------------------------------------------------------------
# Per-iteration event stream provider (with compaction retry)
# ---------------------------------------------------------------------------


async def event_stream_with_context_retry(
    turn_tools: list[dict],
    conversation_messages: list[MessageParam],
    llm_provider: LLMProvider,
    chat_user_id: str,
    chat_id: str,
    system_prompt: str,
    compactor: ConversationCompactor,
    latest_compaction_summary: str | None,
    summarizer_context_window_tokens: int,
) -> AsyncIterator[MessageStreamEvent]:
    """Stream events from the LLM provider with one automatic compaction retry
    on context-overflow errors.

    ``conversation_messages`` is mutated in-place when a compaction retry
    replaces the full history with a compacted version.
    """
    for llm_attempt in range(2):
        tracker = UsageTracker(
            UsageRepository(),
            UsageContext(
                user_id=chat_user_id,
                model_id=llm_provider.model_record_id,
                model_name=llm_provider.model_name,
                provider_type=llm_provider.provider_type,
                purpose=UsagePurpose.CHAT,
                chat_id=chat_id,
            ),
        )
        raw_stream: AsyncStream[MessageStreamEvent] = llm_provider.stream_response(
            prompt="",
            messages=conversation_messages,
            tools=turn_tools,
            max_tokens=DEFAULT_MAX_TOKENS,
            temperature=DEFAULT_TEMPERATURE,
            top_p=DEFAULT_TOP_P,
            system_prompt=system_prompt,
        )
        emitted_event = False
        try:
            async for wrapped_event in tracker.wrap_stream(raw_stream):
                emitted_event = True
                yield wrapped_event
            tracker.save()
            return
        except ProviderError as e:
            if e.is_context_overflow and llm_attempt == 0 and not emitted_event:
                logger.warning(
                    "Chat %s hit provider context limit; retrying once after forced compaction",
                    chat_id,
                )
                conversation_messages[:] = await compactor.compact_conversation(
                    chat_id,
                    conversation_messages,
                    previous_summary=latest_compaction_summary,
                    summarizer_context_window_tokens=summarizer_context_window_tokens,
                )
                continue
            raise


# ---------------------------------------------------------------------------
# Message-content helpers
# ---------------------------------------------------------------------------


def message_content_blocks(message: MessageParam) -> list[ContentBlockParam]:
    content = message["content"]
    if isinstance(content, str):
        return []
    return list(cast(Iterable[ContentBlockParam], content))


def tool_use_blocks(message: MessageParam) -> list[ToolUseBlockParam]:
    if message.get("role") != "assistant":
        return []
    return [
        cast(ToolUseBlockParam, block)
        for block in message_content_blocks(message)
        if block["type"] == "tool_use"
    ]


def tool_result_ids(message: MessageParam) -> set[str]:
    if message.get("role") != "user":
        return set()
    return {
        cast(ToolResultBlockParam, block)["tool_use_id"]
        for block in message_content_blocks(message)
        if block["type"] == "tool_result"
    }


def _interrupted_tool_result(tool_use: ToolUseBlockParam) -> ToolResultBlockParam:
    return ToolResultBlockParam(
        type="tool_result",
        tool_use_id=tool_use["id"],
        content=[
            {
                "type": "text",
                "text": (
                    f"Tool call {tool_use['name']} did not complete because the previous response was interrupted. "
                    "Treat this tool call as failed and retry it if the result is still needed."
                ),
            }
        ],
        is_error=True,
    )


def repair_interrupted_tool_calls(
    messages: list[MessageParam],
) -> tuple[list[MessageParam], int]:
    """Insert ToolResultBlockParam placeholders for tool calls whose
    responses were lost (interrupted stream)."""
    repaired: list[MessageParam] = []
    repair_count = 0

    for idx, message in enumerate(messages):
        tool_uses = tool_use_blocks(message)
        if not tool_uses:
            repaired.append(message)
            continue

        next_message = messages[idx + 1] if idx + 1 < len(messages) else None
        answered_ids = tool_result_ids(next_message) if next_message else set()
        missing = [
            tool_use for tool_use in tool_uses if tool_use["id"] not in answered_ids
        ]

        repaired.append(message)
        if not missing:
            continue

        missing_results = [_interrupted_tool_result(tool_use) for tool_use in missing]
        if answered_ids and next_message is not None:
            content = next_message["content"]
            if isinstance(content, list):
                next_message = cast(MessageParam, dict(next_message))
                next_message["content"] = [*content, *missing_results]
                messages[idx + 1] = next_message
                repair_count += len(missing_results)
                continue

        repaired.append(MessageParam(role="user", content=missing_results))
        repair_count += len(missing_results)

    return repaired, repair_count


def drop_empty_assistant_messages(
    messages: list[MessageParam],
) -> list[MessageParam]:
    """Remove assistant messages with no content and no tool_use blocks."""
    kept: list[MessageParam] = []
    for message in messages:
        if message.get("role") == "assistant":
            blocks = message_content_blocks(message)
            has_tool_use = any(b.get("type") == "tool_use" for b in blocks)
            has_content = bool(blocks) and any(
                b.get("type") == "text" and b.get("text") for b in blocks
            )
            if not has_tool_use and not has_content:
                continue
        kept.append(message)
    return kept


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


def _copy_provider_extras(src: object, dst: dict, keys: tuple[str, ...]) -> None:
    """Copy provider-declared sidecar fields off a Pydantic content_block
    instance onto its persisted TypedDict block."""
    for key in keys:
        value = getattr(src, key, None)
        if value is not None:
            dst[key] = value  # type: ignore[typeddict-unknown-key]


async def active_path_tool_call_ids(messages_repo, chat_id: str) -> set[str]:
    active_path = await messages_repo.get_active_path(chat_id)
    return {
        tool_use["id"]
        for message in active_path
        for tool_use in tool_use_blocks(MessageParam(**message.message))
    }


# ---------------------------------------------------------------------------
# The main generator
# ---------------------------------------------------------------------------


async def stream_generator(
    chat_id: str,
    redis_client,
    messages: list[MessageParam],
    llm_provider: LLMProvider,
    chat_user_id: str,
    *,
    tool_user_id: str | None = None,
    user_email: str | None = None,
    user_configuration=None,
    tool_skip_perm: bool = False,
    system_prompt: str,
    registry: ToolRegistry,
    always_on_handlers: list[ToolHandler],
    connector_handler: ConnectorToolHandler | None,
    loaded_toolsets: set[str],
    compactor: ConversationCompactor,
    latest_compaction_summary: str | None = None,
    summarizer_context_window_tokens: int = 0,
    memory_provider=None,
    memory_write_key: str | None = None,
    effective_mode=MemoryMode.OFF,
    approvals_repo=None,
    pending: list | None = None,
    pending_oauth: list | None = None,
    original_user_query: str | None = None,
) -> AsyncIterator[str]:
    """Core agent loop: yields SSE event strings.

    This is the extracted, module-level version of the generator that was
    formerly defined inside ``stream_chat``.  All closure-captured locals are
    now explicit parameters.
    """
    try:
        conversation_messages = messages.copy()
        content_blocks: list[TextBlockParam | ToolUseBlockParam] = []
        content_blocks_finalized = False

        # ----- Pending approval / OAuth resume ---------------------------------
        if pending or pending_oauth:
            if pending_oauth:
                logger.info(f"Resuming from pending oauth for chat {chat_id}")
                oauth_by_tool_call_id = {
                    approval.tool_call_id: approval
                    for approval in pending_oauth
                    if approval.tool_call_id is not None
                }
                answered_ids = (
                    tool_result_ids(conversation_messages[-1])
                    if conversation_messages
                    else set()
                )
                tool_calls_to_resume: list[ToolUseBlockParam] = []
                for message in reversed(conversation_messages):
                    tool_uses = tool_use_blocks(message)
                    if not tool_uses:
                        answered_ids.update(tool_result_ids(message))
                        continue
                    tool_calls_to_resume = [
                        tool_use
                        for tool_use in tool_uses
                        if tool_use["id"] not in answered_ids
                    ]
                    break

                context = ToolContext(
                    chat_id=chat_id,
                    user_id=tool_user_id,
                    user_email=user_email,
                    user_configuration=user_configuration,
                    skip_permission_check=tool_skip_perm,
                )
                tool_results: list[ToolResultBlockParam] = []
                completed_oauth_ids: list[str] = []
                for tool_call in tool_calls_to_resume:
                    result = await registry.execute(
                        tool_call["name"], tool_call["input"], context
                    )
                    if result.oauth_required is not None:
                        payload = result.oauth_required
                        if tool_results:
                            tool_result_message = MessageParam(
                                role="user", content=tool_results
                            )
                            conversation_messages.append(tool_result_message)
                            for tool_result in tool_results:
                                yield (
                                    f"event: message\ndata: "
                                    f"{json.dumps(tool_result)}\n\n"
                                )
                            yield (
                                f"event: save_message\ndata: "
                                f"{json.dumps(tool_result_message)}\n\n"
                            )
                        for aid in completed_oauth_ids:
                            await approvals_repo.update_status(
                                aid, ToolApprovalStatus.COMPLETED, chat_user_id
                            )
                        yield (
                            f"event: oauth_required\ndata: "
                            f"{json.dumps(oauth_event(tool_call['id'], tool_call['name'], payload.source_id, payload.source_type, payload.provider, payload.oauth_start_url))}\n\n"
                        )
                        yield end_of_stream(
                            EndOfStreamReason.OAUTH_REQUIRED, message="OAuth required"
                        )
                        return

                    tool_results.append(
                        ToolResultBlockParam(
                            type="tool_result",
                            tool_use_id=tool_call["id"],
                            content=result.content,
                            is_error=result.is_error,
                        )
                    )
                    approval = oauth_by_tool_call_id.get(tool_call["id"])
                    if approval:
                        completed_oauth_ids.append(approval.id)

                if tool_results:
                    tool_result_message = MessageParam(
                        role="user", content=tool_results
                    )
                    conversation_messages.append(tool_result_message)
                    for tool_result in tool_results:
                        yield f"event: message\ndata: {json.dumps(tool_result)}\n\n"
                    yield f"event: save_message\ndata: {json.dumps(tool_result_message)}\n\n"

                for aid in completed_oauth_ids:
                    await approvals_repo.update_status(
                        aid, ToolApprovalStatus.COMPLETED, chat_user_id
                    )

                if tool_results:
                    pending_oauth = []
                else:
                    intervention = pending_oauth[0]
                    ev = oauth_event(
                        intervention.tool_call_id,
                        intervention.tool_name,
                        intervention.source_id,
                        intervention.source_type,
                        intervention.provider,
                        intervention.oauth_start_url,
                    )
                    yield f"event: oauth_required\ndata: {json.dumps(ev)}\n\n"
                    yield end_of_stream(
                        EndOfStreamReason.OAUTH_REQUIRED, message="OAuth required"
                    )
                    return

            logger.info(f"Resuming from pending approval batch for chat {chat_id}")
            if any(
                approval.status == ToolApprovalStatus.PENDING
                for approval in pending or []
            ):
                yield (
                    f"event: approval_required\ndata: "
                    f"{json.dumps(approval_required_event(pending or [], tool_use_blocks))}\n\n"
                )
                yield end_of_stream(
                    EndOfStreamReason.APPROVAL_REQUIRED, message="Approval required"
                )
                return

            approvals_by_tool_call_id = {
                approval.tool_call_id: approval
                for approval in (pending or [])
                if approval.tool_call_id is not None
            }
            answered_ids = (
                tool_result_ids(conversation_messages[-1])
                if conversation_messages
                else set()
            )
            tool_calls_to_resume: list[ToolUseBlockParam] = []
            for message in reversed(conversation_messages):
                tool_uses = tool_use_blocks(message)
                if not tool_uses:
                    answered_ids.update(tool_result_ids(message))
                    continue
                tool_calls_to_resume = [
                    tool_use
                    for tool_use in tool_uses
                    if tool_use["id"] not in answered_ids
                ]
                break

            context = ToolContext(
                chat_id=chat_id,
                user_id=tool_user_id,
                user_email=user_email,
                user_configuration=user_configuration,
                skip_permission_check=tool_skip_perm,
            )
            tool_results: list[ToolResultBlockParam] = []
            completed_approval_ids: list[str] = []
            for tool_call in tool_calls_to_resume:
                approval = approvals_by_tool_call_id.get(tool_call["id"])
                if approval and approval.status == ToolApprovalStatus.DENIED:
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
                    completed_approval_ids.append(approval.id)
                else:
                    result = await registry.execute(
                        tool_call["name"], tool_call["input"], context
                    )
                    if result.oauth_required is not None:
                        payload = result.oauth_required
                        if approval:
                            completed_approval_ids.append(approval.id)
                        await approvals_repo.create_pending(
                            chat_id=chat_id,
                            user_id=chat_user_id,
                            tool_name=tool_call["name"],
                            tool_input=tool_call["input"],
                            tool_call_id=tool_call["id"],
                            approval_type=ToolApprovalType.OAUTH,
                            source_id=payload.source_id,
                            source_type=payload.source_type,
                            provider=payload.provider,
                            oauth_start_url=payload.oauth_start_url,
                        )
                        if tool_results:
                            tool_result_message = MessageParam(
                                role="user", content=tool_results
                            )
                            conversation_messages.append(tool_result_message)
                            for tool_result in tool_results:
                                yield (
                                    f"event: message\ndata: "
                                    f"{json.dumps(tool_result)}\n\n"
                                )
                            yield (
                                f"event: save_message\ndata: "
                                f"{json.dumps(tool_result_message)}\n\n"
                            )
                        for aid in completed_approval_ids:
                            await approvals_repo.update_status(
                                aid, ToolApprovalStatus.COMPLETED, chat_user_id
                            )
                        yield (
                            f"event: oauth_required\ndata: "
                            f"{json.dumps(oauth_event(tool_call['id'], tool_call['name'], payload.source_id, payload.source_type, payload.provider, payload.oauth_start_url))}\n\n"
                        )
                        yield end_of_stream(
                            EndOfStreamReason.OAUTH_REQUIRED, message="OAuth required"
                        )
                        return

                    tool_result = ToolResultBlockParam(
                        type="tool_result",
                        tool_use_id=tool_call["id"],
                        content=result.content,
                        is_error=result.is_error,
                    )
                    if approval:
                        completed_approval_ids.append(approval.id)

                tool_results.append(tool_result)
                yield f"event: message\ndata: {json.dumps(tool_result)}\n\n"

            if tool_results:
                tool_result_message = MessageParam(role="user", content=tool_results)
                conversation_messages.append(tool_result_message)
                yield (
                    f"event: save_message\ndata: "
                    f"{json.dumps(tool_result_message)}\n\n"
                )
            for aid in completed_approval_ids:
                await approvals_repo.update_status(
                    aid, ToolApprovalStatus.COMPLETED, chat_user_id
                )

        logger.info(
            f"Starting conversation with {len(conversation_messages)} initial messages"
        )

        # Extract the first user message for caching purposes
        original_user_query_final = original_user_query
        if original_user_query_final is None:
            for msg in conversation_messages:
                if msg.get("role") == "user":
                    raw = msg.get("content", "")
                    if isinstance(raw, str):
                        original_user_query_final = raw
                        break
                    elif isinstance(raw, list):
                        parts = [
                            b.get("text", "")
                            for b in raw
                            if isinstance(b, dict) and b.get("type") == "text"
                        ]
                        if parts:
                            original_user_query_final = " ".join(parts)
                            break

        context = ToolContext(
            chat_id=chat_id,
            user_id=tool_user_id,
            user_email=user_email,
            user_configuration=user_configuration,
            original_user_query=original_user_query_final,
            skip_permission_check=tool_skip_perm,
        )

        usage_repo = UsageRepository()
        assistant_message: MessageParam | None = None

        # ----- Main agent loop -------------------------------------------------
        for iteration in range(AGENT_MAX_ITERATIONS):
            if await is_run_cancelled(redis_client, chat_id):
                logger.info(f"Run cancelled, stopping stream for chat {chat_id}")
                break

            logger.info(f"Iteration {iteration + 1}/{AGENT_MAX_ITERATIONS}")
            content_blocks = []
            content_blocks_finalized = False
            provider_extras = llm_provider.PERSISTED_BLOCK_EXTRAS

            turn_tools = build_turn_tools(
                always_on_handlers,
                connector_handler,
                loaded_toolsets,
            )

            logger.info("Sending request to LLM provider")

            stream = event_stream_with_context_retry(
                turn_tools,
                conversation_messages,
                llm_provider,
                chat_user_id,
                chat_id,
                system_prompt,
                compactor,
                latest_compaction_summary,
                summarizer_context_window_tokens,
            )

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
                    if await is_run_cancelled(redis_client, chat_id):
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
                            content_blocks.append(TextBlockParam(type="text", text=""))
                        text_block = cast(TextBlockParam, content_blocks[event.index])
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
                        text_block = cast(TextBlockParam, content_blocks[event.index])
                        if "citations" not in text_block or not text_block["citations"]:
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

                logger.debug(f"Yielding event to client: {event.to_json(indent=None)}")
                yield f"event: message\ndata: {event.to_json(indent=None)}\n\n"

                if message_stop_received:
                    break

            # ----- Per-iteration post-processing --------------------------------
            if cancelled:
                assistant_message = partial_assistant_message(content_blocks)
                if assistant_message is not None:
                    conversation_messages.append(assistant_message)
                    yield f"event: save_message\ndata: {json.dumps(assistant_message)}\n\n"
                break

            tool_calls = [b for b in content_blocks if b["type"] == "tool_use"]
            parse_errors = parse_tool_call_inputs(
                cast(list[ToolUseBlockParam], tool_calls)
            )

            assistant_message = MessageParam(role="assistant", content=content_blocks)
            conversation_messages.append(assistant_message)
            yield f"event: save_message\ndata: {json.dumps(assistant_message)}\n\n"
            content_blocks_finalized = True

            if parse_errors:
                tool_result_message = MessageParam(role="user", content=parse_errors)
                conversation_messages.append(tool_result_message)
                for pe in parse_errors:
                    yield f"event: message\ndata: {json.dumps(pe)}\n\n"
                yield f"event: save_message\ndata: {json.dumps(tool_result_message)}\n\n"
                continue

            if not tool_calls:
                logger.info(
                    f"No tool calls in iteration {iteration + 1}, completing response"
                )
                break

            logger.info(f"Processing {len(tool_calls)} tool calls")

            if await is_run_cancelled(redis_client, chat_id):
                logger.info(f"Run cancelled before tool execution for chat {chat_id}")
                break

            # ----- Approval checks before tool execution ------------------------
            approval_required: list[ToolApproval] = []
            for tool_call in tool_calls:
                tool_name = tool_call["name"]
                tool_input = tool_call["input"]
                if registry.requires_approval(tool_name):
                    logger.info(f"Tool {tool_name} requires approval")
                    approval = await approvals_repo.create_pending(
                        chat_id=chat_id,
                        user_id=chat_user_id,
                        tool_name=tool_name,
                        tool_input=tool_input,
                        tool_call_id=tool_call["id"],
                        approval_type=ToolApprovalType.APPROVAL,
                        source_id=tool_input.get("source_id"),
                        source_type=tool_input.get("source_type"),
                    )
                    logger.info(
                        f"Saved pending approval {approval.id} for chat {chat_id}"
                    )
                    approval_required.append(approval)

            if approval_required:
                logger.info(
                    f"Pausing stream for {len(approval_required)} approval-required tool call(s)"
                )
                yield (
                    f"event: approval_required\ndata: "
                    f"{json.dumps(approval_required_event(approval_required, tool_use_blocks))}\n\n"
                )
                yield end_of_stream(
                    EndOfStreamReason.APPROVAL_REQUIRED, message="Approval required"
                )
                return

            # ----- Execute tools -----------------------------------------------
            tool_results = []
            for tool_call in tool_calls:
                tool_name = tool_call["name"]
                tool_input = tool_call["input"]

                result = await registry.execute(tool_name, tool_input, context)
                if result.oauth_required is not None:
                    payload = result.oauth_required
                    await approvals_repo.create_pending(
                        chat_id=chat_id,
                        user_id=chat_user_id,
                        tool_name=tool_name,
                        tool_input=tool_input,
                        tool_call_id=tool_call["id"],
                        approval_type=ToolApprovalType.OAUTH,
                        source_id=payload.source_id,
                        source_type=payload.source_type,
                        provider=payload.provider,
                        oauth_start_url=payload.oauth_start_url,
                    )
                    logger.info(f"Saved pending oauth approval for chat {chat_id}")
                    yield (
                        f"event: oauth_required\ndata: "
                        f"{json.dumps(oauth_event(tool_call['id'], tool_name, payload.source_id, payload.source_type, payload.provider, payload.oauth_start_url))}\n\n"
                    )
                    yield end_of_stream(
                        EndOfStreamReason.OAUTH_REQUIRED, message="OAuth required"
                    )
                    return

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
            yield f"event: save_message\ndata: {json.dumps(tool_result_message)}\n\n"

        # ----- Memory write (fire-and-forget) ----------------------------------
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
                            raw = " ".join(
                                b.get("text", "")
                                for b in raw
                                if isinstance(b, dict) and b.get("type") == "text"
                            )
                        if not raw:
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
                            MessageParam(role="assistant", content=assistant_content),
                        ]
                        asyncio.create_task(
                            memory_provider.add(messages=turn, key=memory_write_key)
                        )
            except Exception as e:
                logger.warning(f"Memory write setup failed for chat {chat_id}: {e}")

        yield end_of_stream(EndOfStreamReason.COMPLETED, message="Stream ended")

    except asyncio.CancelledError:
        logger.info(f"Stream cancelled for chat {chat_id}")
        if not content_blocks_finalized:
            partial = partial_assistant_message(content_blocks)
            if partial is not None:
                conversation_messages.append(partial)
                yield f"event: save_message\ndata: {json.dumps(partial)}\n\n"
        raise
    except Exception as e:
        logger.error(f"Failed to generate AI response with tools: {e}", exc_info=True)
        if not content_blocks_finalized:
            partial = partial_assistant_message(content_blocks)
            if partial is not None:
                conversation_messages.append(partial)
                yield f"event: save_message\ndata: {json.dumps(partial)}\n\n"
        yield stream_error_sse(e)
