"""Agent loop generator for the chat streaming pipeline.

Owns ``stream_generator``, the per-iteration event handler, and the message-
content helpers that the agent loop uses to inspect/repair conversation history.
"""

from __future__ import annotations

import asyncio
import json
import logging
from collections.abc import AsyncIterator, Iterable
from typing import cast

from anthropic import MessageStreamEvent
from anthropic.types import (
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
from services.citations import (
    CITATION_INSTRUCTION,
    CitableRef,
    CitationProcessor,
    CitationStreamProcessor,
)
from services.compaction import ConversationCompactor
from services.usage import UsageContext, UsagePurpose, UsageTracker
from streaming.persist import (
    EndOfStreamReason,
    OAuthRequiredEvent,
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
from tools.turn_builder import build_turn_tools

logger = logging.getLogger(__name__)

_EMPTY_RESPONSE_RECOVERY_PROMPT = (
    "Continue the original request. If the last result discovered or loaded a tool, "
    "call the appropriate available tool now. Do not stop without either making the "
    "next tool call or giving the user a clear explanation."
)


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
        provider_messages = conversation_messages
        provider_system_prompt = system_prompt
        citable_index: dict[int, CitableRef] = {}
        if not llm_provider.supports_citations:
            citable_index = CitationProcessor.build_citable_index(conversation_messages)
            if citable_index:
                provider_messages = CitationProcessor.prepare_messages(
                    conversation_messages, citable_index
                )
                provider_system_prompt = (system_prompt or "") + CITATION_INSTRUCTION

        raw_stream = llm_provider.stream_response(
            prompt="",
            messages=provider_messages,
            tools=turn_tools,
            max_tokens=DEFAULT_MAX_TOKENS,
            temperature=DEFAULT_TEMPERATURE,
            top_p=DEFAULT_TOP_P,
            system_prompt=provider_system_prompt,
        )
        processed_stream = tracker.wrap_stream(raw_stream)
        if citable_index:
            processed_stream = CitationStreamProcessor(citable_index).process(
                processed_stream
            )

        emitted_event = False
        try:
            async for wrapped_event in processed_stream:
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


def unanswered_tool_calls(messages: list[MessageParam]) -> list[ToolUseBlockParam]:
    answered_ids = {
        tool_result_id
        for message in messages
        for tool_result_id in tool_result_ids(message)
    }
    return [
        tool_use
        for message in messages
        for tool_use in tool_use_blocks(message)
        if tool_use["id"] not in answered_ids
    ]


def latest_intervention_tool_batch_ids(
    messages: list[MessageParam], intervention_tool_call_ids: set[str]
) -> set[str]:
    for message in reversed(messages):
        tool_calls = tool_use_blocks(message)
        tool_call_ids = {tool_call["id"] for tool_call in tool_calls}
        if tool_call_ids & intervention_tool_call_ids:
            return tool_call_ids
    return set()


def coalesce_adjacent_tool_result_messages(
    messages: list[MessageParam],
) -> list[MessageParam]:
    coalesced: list[MessageParam] = []
    for message in messages:
        blocks = message_content_blocks(message)
        is_tool_result_message = (
            message.get("role") == "user"
            and bool(blocks)
            and all(block["type"] == "tool_result" for block in blocks)
        )
        if is_tool_result_message and coalesced:
            previous_blocks = message_content_blocks(coalesced[-1])
            previous_is_tool_result_message = (
                coalesced[-1].get("role") == "user"
                and bool(previous_blocks)
                and all(block["type"] == "tool_result" for block in previous_blocks)
            )
            if previous_is_tool_result_message:
                coalesced[-1] = MessageParam(
                    role="user", content=[*previous_blocks, *blocks]
                )
                continue
        coalesced.append(message)
    return coalesced


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
    preserve_tool_call_ids: set[str] | None = None,
) -> tuple[list[MessageParam], int]:
    """Insert ToolResultBlockParam placeholders for tool calls whose
    responses were lost (interrupted stream)."""
    repaired: list[MessageParam] = []
    repair_count = 0
    preserved_ids = preserve_tool_call_ids or set()

    for idx, message in enumerate(messages):
        tool_uses = tool_use_blocks(message)
        if not tool_uses:
            repaired.append(message)
            continue

        next_message = messages[idx + 1] if idx + 1 < len(messages) else None
        answered_ids = tool_result_ids(next_message) if next_message else set()
        missing = [
            tool_use
            for tool_use in tool_uses
            if tool_use["id"] not in answered_ids
            and tool_use["id"] not in preserved_ids
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


def _copy_provider_extras(src: object, dst: dict, keys: tuple[str, ...]) -> None:
    """Copy provider-declared sidecar fields off a Pydantic content_block
    instance onto its persisted TypedDict block."""
    for key in keys:
        value = getattr(src, key, None)
        if value is not None:
            dst[key] = value  # type: ignore[typeddict-unknown-key]


async def active_path_tool_call_ids(messages_repo, chat_id: str) -> set[str]:
    active_path = await messages_repo.get_active_path(chat_id)
    messages = [MessageParam(**message.message) for message in active_path]
    return {tool_use["id"] for tool_use in unanswered_tool_calls(messages)}


def oauth_event_from_approval(approval: ToolApproval) -> OAuthRequiredEvent:
    if (
        approval.tool_call_id is None
        or approval.source_id is None
        or approval.source_type is None
        or approval.provider is None
        or approval.oauth_start_url is None
    ):
        raise ValueError(f"OAuth approval {approval.id} is missing required metadata")
    return oauth_event(
        approval.id,
        approval.tool_call_id,
        approval.tool_name,
        approval.source_id,
        approval.source_type,
        approval.provider,
        approval.oauth_start_url,
    )


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
    pending_interventions: list[ToolApproval] | None = None,
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

        approval_interventions_by_tool_call_id = {
            approval.tool_call_id: approval
            for approval in pending_interventions or []
            if approval.tool_call_id is not None
            and approval.approval_type == ToolApprovalType.APPROVAL
        }
        oauth_interventions_by_tool_call_id = {
            approval.tool_call_id: approval
            for approval in pending_interventions or []
            if approval.tool_call_id is not None
            and approval.approval_type == ToolApprovalType.OAUTH
        }
        unanswered_calls = unanswered_tool_calls(conversation_messages)
        unanswered_ids = {tool_call["id"] for tool_call in unanswered_calls}

        approved_oauth_keys = {
            (approval.source_id, approval.source_type, approval.provider)
            for tool_call_id, approval in oauth_interventions_by_tool_call_id.items()
            if tool_call_id in unanswered_ids
            and approval.status == ToolApprovalStatus.APPROVED
        }
        blocked_oauth = next(
            (
                approval
                for tool_call_id, approval in oauth_interventions_by_tool_call_id.items()
                if tool_call_id in unanswered_ids
                and approval.status == ToolApprovalStatus.PENDING
                and (approval.source_id, approval.source_type, approval.provider)
                not in approved_oauth_keys
            ),
            None,
        )
        if blocked_oauth is not None:
            yield sse_event("oauth_required", oauth_event_from_approval(blocked_oauth))
            yield end_of_stream(
                EndOfStreamReason.OAUTH_REQUIRED, message="OAuth required"
            )
            return

        blocked_approvals = [
            approval
            for tool_call_id, approval in approval_interventions_by_tool_call_id.items()
            if tool_call_id in unanswered_ids
            and approval.status == ToolApprovalStatus.PENDING
        ]
        if blocked_approvals:
            yield sse_event(
                "approval_required",
                approval_required_event(blocked_approvals, tool_use_blocks),
            )
            yield end_of_stream(
                EndOfStreamReason.APPROVAL_REQUIRED, message="Approval required"
            )
            return

        intervention_tool_call_ids = set(approval_interventions_by_tool_call_id) | set(
            oauth_interventions_by_tool_call_id
        )
        resumable_batch_ids = latest_intervention_tool_batch_ids(
            conversation_messages, intervention_tool_call_ids
        )
        resumable_tool_calls = [
            tool_call
            for tool_call in unanswered_calls
            if tool_call["id"] in resumable_batch_ids
        ]

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

        if approvals_repo is None:
            raise ValueError("Tool approvals repository is required")

        assistant_message: MessageParam | None = None

        # ----- Main agent loop -------------------------------------------------
        model_iteration = 0
        empty_response_retries = 0
        loop_passes = AGENT_MAX_ITERATIONS + (1 if resumable_tool_calls else 0)
        for _ in range(loop_passes):
            if await is_run_cancelled(redis_client, chat_id):
                logger.info(f"Run cancelled, stopping stream for chat {chat_id}")
                break

            content_blocks = []
            content_blocks_finalized = False
            parse_errors_by_tool_call_id: dict[str, ToolResultBlockParam] = {}
            if resumable_tool_calls:
                tool_calls = resumable_tool_calls
                resumable_tool_calls = []
                logger.info("Processing %s resumed tool calls", len(tool_calls))
            else:
                model_iteration += 1
                logger.info(
                    "Model iteration %s/%s", model_iteration, AGENT_MAX_ITERATIONS
                )
                conversation_messages = coalesce_adjacent_tool_result_messages(
                    conversation_messages
                )
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
                pending_message_start_sse: str | None = None
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
                            citations.append(
                                CitationProcessor.convert_delta_to_param(event.delta)
                            )

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

                    event_json = event.to_json(indent=None)
                    event_sse = f"event: message\ndata: {event_json}\n\n"
                    if event.type == "message_start":
                        # Hold this until the provider emits actual content. If
                        # it immediately stops, the retry below stays invisible
                        # and the persistence wrapper does not create an empty
                        # assistant row.
                        pending_message_start_sse = event_sse
                    elif event.type == "message_stop" and not content_blocks:
                        pass
                    else:
                        if pending_message_start_sse is not None:
                            yield pending_message_start_sse
                            pending_message_start_sse = None
                        logger.debug("Yielding event to client: %s", event_json)
                        yield event_sse

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
                has_text = any(
                    b["type"] == "text" and str(b.get("text", "")).strip()
                    for b in content_blocks
                )
                if not tool_calls and not has_text and empty_response_retries < 1:
                    empty_response_retries += 1
                    logger.warning(
                        "Provider returned an empty response in iteration %s; "
                        "retrying once with a continuation prompt",
                        model_iteration,
                    )
                    conversation_messages.append(
                        MessageParam(
                            role="user", content=_EMPTY_RESPONSE_RECOVERY_PROMPT
                        )
                    )
                    continue
                parse_errors = parse_tool_call_inputs(
                    cast(list[ToolUseBlockParam], tool_calls)
                )
                parse_errors_by_tool_call_id = {
                    error["tool_use_id"]: error for error in parse_errors
                }

                assistant_message = MessageParam(
                    role="assistant", content=content_blocks
                )
                conversation_messages.append(assistant_message)
                yield f"event: save_message\ndata: {json.dumps(assistant_message)}\n\n"
                content_blocks_finalized = True

                if not tool_calls:
                    logger.info(
                        f"No tool calls in iteration {model_iteration}, completing response"
                    )
                    break

                logger.info(f"Processing {len(tool_calls)} tool calls")

            if await is_run_cancelled(redis_client, chat_id):
                logger.info(f"Run cancelled before tool execution for chat {chat_id}")
                break

            # Preflight credentials before asking for user approval. This keeps
            # blocked write tools from showing an approval card before the user
            # has connected the OAuth credential needed to run them.
            oauth_required: list[ToolApproval] = []
            for tool_call in tool_calls:
                if tool_call["id"] in parse_errors_by_tool_call_id:
                    continue
                payload = await registry.check_oauth_required(
                    tool_call["name"], tool_call["input"], context
                )
                if payload is None:
                    continue
                oauth_intervention = oauth_interventions_by_tool_call_id.get(
                    tool_call["id"]
                )
                if oauth_intervention is None:
                    oauth_intervention = await approvals_repo.create_pending(
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
                    oauth_interventions_by_tool_call_id[tool_call["id"]] = (
                        oauth_intervention
                    )
                elif oauth_intervention.status == ToolApprovalStatus.APPROVED:
                    await approvals_repo.update_status(
                        oauth_intervention.id,
                        ToolApprovalStatus.PENDING,
                        chat_user_id,
                    )
                oauth_required.append(oauth_intervention)

            if oauth_required:
                for oauth_intervention in oauth_required:
                    yield sse_event(
                        "oauth_required", oauth_event_from_approval(oauth_intervention)
                    )
                yield end_of_stream(
                    EndOfStreamReason.OAUTH_REQUIRED, message="OAuth required"
                )
                return

            # Preflight approval for credentials-ready tools. Existing
            # approved/denied interventions are reused on resume.
            approval_required: list[ToolApproval] = []
            for tool_call in tool_calls:
                if tool_call["id"] in parse_errors_by_tool_call_id:
                    continue
                if not registry.requires_approval(tool_call["name"]):
                    continue
                approval = approval_interventions_by_tool_call_id.get(tool_call["id"])
                if approval is None:
                    tool_input = tool_call["input"]
                    approval = await approvals_repo.create_pending(
                        chat_id=chat_id,
                        user_id=chat_user_id,
                        tool_name=tool_call["name"],
                        tool_input=tool_input,
                        tool_call_id=tool_call["id"],
                        approval_type=ToolApprovalType.APPROVAL,
                        source_id=tool_input.get("source_id"),
                        source_type=tool_input.get("source_type"),
                    )
                    approval_interventions_by_tool_call_id[tool_call["id"]] = approval
                if approval.status == ToolApprovalStatus.PENDING:
                    approval_required.append(approval)

            tool_results: list[ToolResultBlockParam] = []
            oauth_required = []
            completed_intervention_ids: set[str] = set()
            for tool_call in tool_calls:
                parse_error = parse_errors_by_tool_call_id.get(tool_call["id"])
                if parse_error is not None:
                    tool_results.append(parse_error)
                    continue
                normal_intervention = approval_interventions_by_tool_call_id.get(
                    tool_call["id"]
                )
                if (
                    normal_intervention is not None
                    and normal_intervention.status == ToolApprovalStatus.PENDING
                ):
                    continue
                if (
                    normal_intervention is not None
                    and normal_intervention.status == ToolApprovalStatus.DENIED
                ):
                    tool_results.append(
                        ToolResultBlockParam(
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
                    )
                    completed_intervention_ids.add(normal_intervention.id)
                    continue

                result = await registry.execute(
                    tool_call["name"], tool_call["input"], context
                )
                if result.oauth_required is not None:
                    payload = result.oauth_required
                    oauth_intervention = oauth_interventions_by_tool_call_id.get(
                        tool_call["id"]
                    )
                    if oauth_intervention is None:
                        oauth_intervention = await approvals_repo.create_pending(
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
                        oauth_interventions_by_tool_call_id[tool_call["id"]] = (
                            oauth_intervention
                        )
                    elif oauth_intervention.status == ToolApprovalStatus.APPROVED:
                        await approvals_repo.update_status(
                            oauth_intervention.id,
                            ToolApprovalStatus.PENDING,
                            chat_user_id,
                        )

                    oauth_required.append(oauth_intervention)
                    continue

                tool_results.append(
                    ToolResultBlockParam(
                        type="tool_result",
                        tool_use_id=tool_call["id"],
                        content=result.content,
                        is_error=result.is_error,
                    )
                )
                if normal_intervention is not None:
                    completed_intervention_ids.add(normal_intervention.id)
                oauth_intervention = oauth_interventions_by_tool_call_id.get(
                    tool_call["id"]
                )
                if oauth_intervention is not None:
                    completed_intervention_ids.add(oauth_intervention.id)

            if tool_results:
                tool_result_message = MessageParam(role="user", content=tool_results)
                conversation_messages.append(tool_result_message)
                for tool_result in tool_results:
                    yield sse_event("message", tool_result)
                yield sse_event("save_message", tool_result_message)
                for approval_id in completed_intervention_ids:
                    await approvals_repo.update_status(
                        approval_id, ToolApprovalStatus.COMPLETED, chat_user_id
                    )

            for oauth_intervention in oauth_required:
                yield sse_event(
                    "oauth_required", oauth_event_from_approval(oauth_intervention)
                )
            if oauth_required:
                yield end_of_stream(
                    EndOfStreamReason.OAUTH_REQUIRED, message="OAuth required"
                )
                return
            if approval_required:
                yield sse_event(
                    "approval_required",
                    approval_required_event(approval_required, tool_use_blocks),
                )
                yield end_of_stream(
                    EndOfStreamReason.APPROVAL_REQUIRED, message="Approval required"
                )
                return

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
