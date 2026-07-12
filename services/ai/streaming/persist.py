"""Persistence and SSE event helpers for the chat streaming pipeline.

Owns pure helpers, typed event definitions, and the ``persist_and_transform``
wrapper that owns the ``current_assistant_message_id`` lifecycle for
early-persisted assistant rows.
"""

from __future__ import annotations

import json
import logging
from collections.abc import Iterable
from enum import Enum
from typing import Any, NotRequired, TypedDict, cast

from anthropic.types import (
    MessageParam,
    TextBlockParam,
    ToolResultBlockParam,
    ToolUseBlockParam,
)

from db.tool_approvals import ToolApproval
from providers import LLMProviderStreamError, ProviderError

logger = logging.getLogger(__name__)


# ---------------------------------------------------------------------------
# SSE event string helpers
# ---------------------------------------------------------------------------


def sse_event(event_type: str, data: object) -> str:
    """Build an SSE ``event:`` / ``data:`` string pair."""
    return f"event: {event_type}\ndata: {json.dumps(data)}\n\n"


def sse_event_type(event_str: str) -> str:
    """Extract the event type from an SSE event string."""
    for line in event_str.split("\n"):
        if line.startswith("event:"):
            return line[len("event:") :].strip()
    return "message"


def sse_event_data(event_str: str) -> str:
    """Extract the JSON data string from an SSE event string."""
    for line in event_str.split("\n"):
        if line.startswith("data:"):
            return line[len("data:") :].strip()
    return ""


# ---------------------------------------------------------------------------
# Typed event envelopes
# ---------------------------------------------------------------------------


class EndOfStreamReason(str, Enum):
    """Typed reason for an ``end_of_stream`` terminal event."""

    COMPLETED = "completed"
    STOPPED = "stopped"
    APPROVAL_REQUIRED = "approval_required"
    OAUTH_REQUIRED = "oauth_required"
    NO_NEW_MESSAGE = "no_new_message"


class EndOfStreamEvent(TypedDict):
    reason: EndOfStreamReason
    message: NotRequired[str]


class StreamErrorEvent(TypedDict):
    message: str
    provider: NotRequired[str]
    model: NotRequired[str]
    statusCode: NotRequired[int | None]


class OAuthRequiredEvent(TypedDict):
    approval_id: str
    tool_call_id: str
    tool_name: str
    source_id: str
    source_type: str
    provider: str
    oauth_start_url: str


class ApprovalRequiredEventItem(TypedDict):
    approval_id: str
    tool_name: str
    tool_input: dict[str, Any]
    tool_call_id: str | None
    source_id: str | None
    source_type: str | None


class ApprovalRequiredEvent(ApprovalRequiredEventItem):
    approvals: list[ApprovalRequiredEventItem]


# ---------------------------------------------------------------------------
# Event-builders
# ---------------------------------------------------------------------------


def _chat_error_message(exc: Exception) -> str:
    if isinstance(exc, ProviderError) and exc.message:
        return f"Failed to generate response: {exc.message}"
    if isinstance(exc, LLMProviderStreamError) and exc.message:
        return f"Failed to generate response: {exc.message}"
    message = str(exc).strip()
    if message:
        return f"Failed to generate response: {message}"
    return "Failed to generate response. Please try again."


def _chat_error_payload(exc: Exception) -> StreamErrorEvent:
    payload: StreamErrorEvent = {"message": _chat_error_message(exc)}
    if isinstance(exc, ProviderError):
        payload["provider"] = exc.provider_type
        payload["model"] = exc.model
        payload["statusCode"] = exc.status_code
    return payload


def stream_error_event(exc: Exception) -> StreamErrorEvent:
    """Build a ``StreamErrorEvent`` from an exception."""
    return _chat_error_payload(exc)


def stream_error_sse(exc: Exception) -> str:
    """Build an SSE ``stream_error`` event string from an exception."""
    return sse_event("stream_error", _chat_error_payload(exc))


def end_of_stream(reason: EndOfStreamReason, *, message: str | None = None) -> str:
    """Build a typed ``end_of_stream`` SSE event string."""
    payload: EndOfStreamEvent = {"reason": reason}
    if message is not None:
        payload["message"] = message
    return sse_event("end_of_stream", payload)


def oauth_event(
    approval_id: str,
    tool_call_id: str,
    tool_name: str,
    source_id: str,
    source_type: str,
    provider: str,
    oauth_start_url: str,
) -> OAuthRequiredEvent:
    return {
        "approval_id": approval_id,
        "tool_call_id": tool_call_id,
        "tool_name": tool_name,
        "source_id": source_id,
        "source_type": source_type,
        "provider": provider,
        "oauth_start_url": oauth_start_url,
    }


def approval_required_event(
    approvals: list[ToolApproval],
    tool_use_blocks_fn,  # callable: list[MessageParam] -> list[ToolUseBlockParam]
) -> ApprovalRequiredEvent:
    """Build an ``ApprovalRequiredEvent`` from a list of pending approvals."""
    first = approvals[0]
    event: ApprovalRequiredEvent = {
        "approval_id": first.id,
        "tool_name": first.tool_name,
        "tool_input": first.tool_input,
        "tool_call_id": first.tool_call_id,
        "source_id": first.source_id,
        "source_type": first.source_type,
        "approvals": [
            {
                "approval_id": approval.id,
                "tool_name": approval.tool_name,
                "tool_input": approval.tool_input,
                "tool_call_id": approval.tool_call_id,
                "source_id": approval.source_id,
                "source_type": approval.source_type,
            }
            for approval in approvals
        ],
    }
    return event


# ---------------------------------------------------------------------------
# Assistant message building
# ---------------------------------------------------------------------------


def partial_assistant_message(
    content_blocks: list[TextBlockParam | ToolUseBlockParam],
) -> MessageParam | None:
    """Build a ``MessageParam`` from partial content blocks.

    Returns ``None`` when there is nothing to persist (no non-empty text or
    tool-use blocks), which lets the caller avoid emitting a ``save_message``
    event for a genuinely empty response.
    """
    persisted_blocks: list[TextBlockParam | ToolUseBlockParam] = []
    for block in content_blocks:
        if block["type"] == "text":
            text_block = cast(TextBlockParam, block)
            if text_block["text"].strip():
                persisted_blocks.append(cast(TextBlockParam, dict(text_block)))
            continue

        tool_block = cast(ToolUseBlockParam, dict(block))
        raw_input = tool_block.get("input")
        if isinstance(raw_input, str):
            try:
                tool_block["input"] = json.loads(raw_input) if raw_input else {}
            except json.JSONDecodeError:
                tool_block["input"] = {}
        persisted_blocks.append(tool_block)

    if not persisted_blocks:
        return None
    return MessageParam(role="assistant", content=persisted_blocks)


def parse_tool_call_inputs(
    tool_calls: list[ToolUseBlockParam],
) -> list[ToolResultBlockParam]:
    """Parse raw JSON input strings on tool-call blocks into Python dicts.

    Returns a list of error ``ToolResultBlockParam`` for any tool calls whose
    input could not be parsed, so the model can retry.
    """
    parse_errors: list[ToolResultBlockParam] = []
    for tool_call in tool_calls:
        raw_input = cast(str, tool_call["input"])
        try:
            tool_call["input"] = json.loads(raw_input)
        except json.JSONDecodeError as e:
            logger.warning(
                "Failed to parse tool call input for %s: %s. Raw input: %s",
                tool_call["name"],
                e,
                raw_input,
            )
            raw_input_preview = raw_input[:4000]
            if len(raw_input) > len(raw_input_preview):
                raw_input_preview += "... [truncated]"
            tool_call["input"] = {}
            parse_errors.append(
                ToolResultBlockParam(
                    type="tool_result",
                    tool_use_id=tool_call["id"],
                    content=[
                        {
                            "type": "text",
                            "text": (
                                f"Invalid JSON in tool input: {e}. "
                                "The tool was not executed. Retry with valid JSON.\n\n"
                                f"Raw tool input:\n{raw_input_preview}"
                            ),
                        }
                    ],
                    is_error=True,
                )
            )
    return parse_errors


# ---------------------------------------------------------------------------
# Stream persistence wrapper
# ---------------------------------------------------------------------------


async def persist_and_transform(gen, chat_id, messages_repo, parent_id):
    """Persist streamed messages before exposing them to the client.

    Assistant rows are created as soon as the provider emits ``message_start``,
    so the browser can use a durable ``chat_messages.id`` for the streaming
    bubble from the beginning.  Tool-result rows are persisted before their
    ``tool_result`` events are forwarded.  This keeps frontend render identities
    and future ``parent_id`` values database-backed even if the run is cancelled
    mid-stream.
    """
    current_assistant_message_id: str | None = None
    buffered_tool_result_events: list[str] = []

    async for event_str in gen:
        event_type = sse_event_type(event_str)
        event_data = sse_event_data(event_str)

        if event_type == "message":
            try:
                message_event = json.loads(event_data)
            except json.JSONDecodeError:
                yield event_str
                continue

            if message_event.get("type") == "message_start":
                try:
                    provider_message = message_event.get("message", {})
                    assistant_message = {
                        "role": provider_message.get("role", "assistant"),
                        "content": provider_message.get("content") or [],
                    }
                    created = await messages_repo.create(
                        chat_id, assistant_message, parent_id=parent_id
                    )
                    current_assistant_message_id = created.id
                    parent_id = created.id
                    message_event.setdefault("message", {})["id"] = created.id
                    event_str = sse_event("message", message_event)
                except Exception as e:
                    logger.error(
                        f"Failed to pre-persist assistant message for chat {chat_id}: {e}",
                        exc_info=True,
                    )
                yield event_str
                continue

            if message_event.get("type") == "tool_result":
                buffered_tool_result_events.append(event_str)
                continue

            yield event_str
            continue

        if event_type == "save_message":
            try:
                message = json.loads(event_data)
                if message.get("role") == "assistant" and current_assistant_message_id:
                    await messages_repo.update_message_content(
                        current_assistant_message_id, message
                    )
                    await messages_repo.update_content_text(
                        current_assistant_message_id, message
                    )
                    current_assistant_message_id = None
                    continue

                created = await messages_repo.create(
                    chat_id, message, parent_id=parent_id
                )
                parent_id = created.id

                if message.get("role") == "user" and buffered_tool_result_events:
                    for buffered_event in buffered_tool_result_events:
                        try:
                            tool_result_event = json.loads(
                                sse_event_data(buffered_event)
                            )
                            tool_result_event["message_id"] = created.id
                            yield sse_event("message", tool_result_event)
                        except json.JSONDecodeError:
                            yield buffered_event
                    buffered_tool_result_events = []
                else:
                    yield f"event: message_id\ndata: {created.id}\n\n"
            except Exception as e:
                logger.error(
                    f"Failed to persist streamed message for chat {chat_id}: {e}",
                    exc_info=True,
                )
            continue

        yield event_str

    # If the generator ended (cancel/error/client disconnect) without a
    # matching save_message, the early-persisted assistant row was never
    # finalized.  With partial content the generator emits ``save_message``
    # itself; reaching here with the id still set means no content was ever
    # committed, so delete the empty row rather than leave an invalid
    # assistant message that would poison the next request's history.
    if current_assistant_message_id is not None:
        try:
            await messages_repo.delete(current_assistant_message_id)
            logger.info(
                f"Deleted unfinalized assistant message "
                f"{current_assistant_message_id} for chat {chat_id}"
            )
        except Exception as e:
            logger.error(
                f"Failed to delete unfinalized assistant message "
                f"{current_assistant_message_id} for chat {chat_id}: {e}"
            )
        current_assistant_message_id = None
