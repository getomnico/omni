"""Tools for searching and reading a user's previous chat sessions."""

from __future__ import annotations

import json
import logging

from anthropic.types import ToolParam
from pydantic import BaseModel, Field, ValidationError, field_validator, model_validator

from db import ChatMessage, ChatsRepository, MessagesRepository
from db.models import ChatSearchHit
from tools.registry import ToolContext, ToolResult

logger = logging.getLogger(__name__)

_SEARCH_TOOL_NAME = "search_chats"
_READ_TOOL_NAME = "read_chat"
_TOOL_NAMES = {_SEARCH_TOOL_NAME, _READ_TOOL_NAME}

_DEFAULT_SEARCH_LIMIT = 10
_MAX_SEARCH_LIMIT = 20
_DEFAULT_READ_LIMIT = 20
_MAX_READ_LIMIT = 50
_MAX_MESSAGE_TEXT = 2_000
_MAX_TOOL_PREVIEW = 400
_MAX_SEARCH_SNIPPET = 240
_MAX_CHAT_TITLE = 500
_MAX_READ_OUTPUT_CHARS = 32_000
_ANCHOR_CONTEXT_MESSAGES = 2


class _SearchChatsInput(BaseModel):
    query: str
    limit: int = Field(default=_DEFAULT_SEARCH_LIMIT, ge=1, le=_MAX_SEARCH_LIMIT)


class _ReadChatInput(BaseModel):
    chat_id: str = Field(min_length=1)
    message_id: str | None = None
    start_seq: int | None = Field(default=None, ge=1)
    end_seq: int | None = Field(default=None, ge=1)
    limit: int = Field(default=_DEFAULT_READ_LIMIT, ge=1, le=_MAX_READ_LIMIT)

    @field_validator("chat_id", "message_id")
    @classmethod
    def _reject_blank_ids(cls, value: str | None) -> str | None:
        if value is not None and not value.strip():
            raise ValueError("IDs cannot be blank")
        return value

    @model_validator(mode="after")
    def _validate_sequence_range(self) -> _ReadChatInput:
        if (
            self.start_seq is not None
            and self.end_seq is not None
            and self.start_seq > self.end_seq
        ):
            raise ValueError("start_seq must not be greater than end_seq")
        return self


_SEARCH_CHATS_TOOL: ToolParam = {
    "name": _SEARCH_TOOL_NAME,
    "description": (
        "Search the current user's previous Omni chat sessions by conversation title "
        "and message content. Use this to find prior decisions, preferences, answers, "
        "or other useful context that is not in the workplace search index. Results "
        "include a chat_id that can be passed to read_chat."
    ),
    "input_schema": {
        "type": "object",
        "properties": {
            "query": {
                "type": "string",
                "description": "The topic, question, decision, or keywords to find in past chats.",
            },
            "limit": {
                "type": "integer",
                "description": "Maximum number of chats to return (default: 10, maximum: 20).",
            },
        },
        "required": ["query"],
    },
}

_READ_CHAT_TOOL: ToolParam = {
    "name": _READ_TOOL_NAME,
    "description": (
        "Read a user's previous chat session. Use chat_id from search_chats. "
        "The result follows one branch of the conversation through its descendant "
        "leaf and is paginated by message sequence number. Pass message_id to select "
        "a branch (especially the matching_message_id returned by search_chats), "
        "and use start_seq or "
        "end_seq for later pages."
    ),
    "input_schema": {
        "type": "object",
        "properties": {
            "chat_id": {
                "type": "string",
                "description": "The chat_id returned by search_chats.",
            },
            "message_id": {
                "type": "string",
                "description": (
                    "Optional branch anchor. Use a matching_message_id from search_chats "
                    "to read the branch containing the matching message. If omitted, "
                    "the active branch is read."
                ),
            },
            "start_seq": {
                "type": "integer",
                "description": "Optional inclusive sequence number at which to start reading.",
            },
            "end_seq": {
                "type": "integer",
                "description": "Optional inclusive sequence number at which to stop reading.",
            },
            "limit": {
                "type": "integer",
                "description": "Maximum number of messages to return (default: 20, maximum: 50).",
            },
        },
        "required": ["chat_id"],
    },
}


def _truncate(text: str, limit: int) -> str:
    if len(text) <= limit:
        return text
    if limit <= 1:
        return "…"[:limit]

    prefix_length = limit - 1
    while True:
        omitted = len(text) - prefix_length
        suffix = f"… ({omitted} more characters)"
        next_prefix_length = limit - len(suffix)
        if next_prefix_length < 0:
            return "…"
        if next_prefix_length == prefix_length:
            return f"{text[:prefix_length]}{suffix}"
        prefix_length = next_prefix_length


def _single_line(text: str) -> str:
    return " ".join(text.split())


def _compact_json(value: object, limit: int) -> str:
    serialized = json.dumps(value, ensure_ascii=False, separators=(",", ":"), default=str)
    return _truncate(serialized, limit)


def _preview_value(value: object, limit: int = _MAX_TOOL_PREVIEW) -> str | None:
    """Extract a short readable preview without returning large tool payloads."""
    if isinstance(value, str):
        compact = _single_line(value)
        return _truncate(compact, limit) if compact else None

    if isinstance(value, list):
        fragments: list[str] = []
        for item in value:
            if isinstance(item, dict):
                block_type = item.get("type")
                if block_type == "text" and isinstance(item.get("text"), str):
                    fragments.append(item["text"])
                elif block_type == "search_result":
                    title = item.get("title")
                    nested = _preview_value(item.get("content"), limit)
                    if isinstance(title, str):
                        fragments.append(title if nested is None else f"{title}: {nested}")
                    elif nested is not None:
                        fragments.append(nested)
                elif block_type == "document" and isinstance(item.get("title"), str):
                    fragments.append(f"Document: {item['title']}")
            elif isinstance(item, str):
                fragments.append(item)
        if fragments:
            return _truncate(_single_line(" ".join(fragments)), limit)

    if isinstance(value, dict) and isinstance(value.get("text"), str):
        return _truncate(_single_line(value["text"]), limit)

    if value is None:
        return None
    return _truncate(_single_line(_compact_json(value, limit)), limit)


def _render_message_content(message: ChatMessage) -> str:
    if message.error is not None:
        error_text = message.error.get("message")
        if isinstance(error_text, str):
            return f"[error: {_truncate(_single_line(error_text), _MAX_TOOL_PREVIEW)}]"
        return "[error]"

    content: object = message.message.get("content")
    if isinstance(content, str):
        return _truncate(content, _MAX_MESSAGE_TEXT)

    if not isinstance(content, list):
        return "[no readable content]"

    parts: list[str] = []
    for block in content:
        if not isinstance(block, dict):
            continue

        block_type = block.get("type")
        if block_type == "text" and isinstance(block.get("text"), str):
            parts.append(block["text"])
        elif block_type == "tool_use":
            name = block.get("name")
            call = f"[Tool call: {name if isinstance(name, str) else 'unknown'}]"
            if "input" in block:
                call += f" {_compact_json(block['input'], _MAX_TOOL_PREVIEW)}"
            parts.append(call)
        elif block_type == "tool_result":
            preview = _preview_value(block.get("content"))
            parts.append("[Tool result]" if preview is None else f"[Tool result: {preview}]")
        elif block_type in {"thinking", "redacted_thinking"}:
            continue
        elif block_type == "image":
            parts.append("[image attachment]")
        elif block_type == "document":
            title = block.get("title")
            parts.append(
                "[document attachment]"
                if not isinstance(title, str)
                else f"[document attachment: {title}]"
            )
        elif isinstance(block_type, str):
            parts.append(f"[{block_type} block]")

    return (
        _truncate("\n".join(parts), _MAX_MESSAGE_TEXT)
        if parts
        else "[no readable content]"
    )


def _invalid_parameters(error: ValidationError) -> ToolResult:
    return ToolResult(
        content=[{"type": "text", "text": f"Invalid parameters: {error}"}],
        is_error=True,
    )


class ChatHistoryToolHandler:
    """Search and read chat history belonging to the current user."""

    def __init__(
        self,
        chats_repo: ChatsRepository | None = None,
        messages_repo: MessagesRepository | None = None,
    ) -> None:
        self._chats = chats_repo if chats_repo is not None else ChatsRepository()
        self._messages = (
            messages_repo if messages_repo is not None else MessagesRepository()
        )

    def get_tools(self) -> list[ToolParam]:
        return [_SEARCH_CHATS_TOOL, _READ_CHAT_TOOL]

    def can_handle(self, tool_name: str) -> bool:
        return tool_name in _TOOL_NAMES

    def requires_approval(self, tool_name: str) -> bool:
        return False

    async def execute(
        self, tool_name: str, tool_input: dict, context: ToolContext
    ) -> ToolResult:
        if tool_name == _SEARCH_TOOL_NAME:
            return await self._execute_search(tool_input, context)
        if tool_name == _READ_TOOL_NAME:
            return await self._execute_read(tool_input, context)
        return ToolResult(
            content=[{"type": "text", "text": f"Unknown chat history tool: {tool_name}"}],
            is_error=True,
        )

    async def _execute_search(
        self, tool_input: dict, context: ToolContext
    ) -> ToolResult:
        try:
            params = _SearchChatsInput.model_validate(tool_input)
        except ValidationError as error:
            return _invalid_parameters(error)

        query = params.query.strip()
        if not query:
            return ToolResult(
                content=[{"type": "text", "text": "Invalid parameters: query cannot be blank"}],
                is_error=True,
            )

        if context.user_id is None:
            return self._unavailable_result()

        try:
            hits = await self._chats.search(
                user_id=context.user_id,
                query=query,
                limit=params.limit,
                exclude_chat_id=context.chat_id,
            )
        except Exception:
            logger.exception("Chat history search failed for user %s", context.user_id)
            return ToolResult(
                content=[{"type": "text", "text": "Chat history search failed."}],
                is_error=True,
            )

        if not hits:
            return ToolResult(
                content=[{"type": "text", "text": "No past chats matched the query."}]
            )

        lines = [f"Found {len(hits)} past chat(s) matching the query:"]
        for hit in hits:
            lines.extend(self._format_search_hit(hit))
        lines.append("Use read_chat with a chat_id to read a result.")
        return ToolResult(content=[{"type": "text", "text": "\n".join(lines)}])

    async def _execute_read(
        self, tool_input: dict, context: ToolContext
    ) -> ToolResult:
        try:
            params = _ReadChatInput.model_validate(tool_input)
        except ValidationError as error:
            return _invalid_parameters(error)

        if context.user_id is None:
            return self._unavailable_result()

        try:
            chat = await self._chats.get(params.chat_id)
            if chat is None or chat.user_id != context.user_id:
                return ToolResult(
                    content=[{"type": "text", "text": "Chat not found."}],
                    is_error=True,
                )

            path = await self._messages.get_active_path(
                params.chat_id, message_id=params.message_id
            )
        except Exception:
            logger.exception(
                "Chat history read failed for chat %s and user %s",
                params.chat_id,
                context.user_id,
            )
            return ToolResult(
                content=[{"type": "text", "text": "Chat could not be read."}],
                is_error=True,
            )

        if not path:
            if params.message_id is not None:
                return ToolResult(
                    content=[
                        {
                            "type": "text",
                            "text": "Message not found in that chat branch.",
                        }
                    ],
                    is_error=True,
                )
            return ToolResult(
                content=[{"type": "text", "text": "That chat has no messages."}]
            )

        branch_start = path[0].message_seq_num
        branch_end = path[-1].message_seq_num
        anchor_index: int | None = None
        if params.message_id is not None:
            anchor_index = next(
                (
                    index
                    for index, message in enumerate(path)
                    if message.id == params.message_id
                ),
                None,
            )
            if anchor_index is None:
                return ToolResult(
                    content=[
                        {
                            "type": "text",
                            "text": "Message not found in that chat branch.",
                        }
                    ],
                    is_error=True,
                )

        if params.start_seq is not None:
            start_seq = params.start_seq
        elif anchor_index is not None:
            context_index = max(0, anchor_index - _ANCHOR_CONTEXT_MESSAGES)
            start_seq = path[context_index].message_seq_num
        else:
            start_seq = branch_start

        end_seq = params.end_seq if params.end_seq is not None else branch_end
        matching_messages = [
            message
            for message in path
            if start_seq <= message.message_seq_num <= end_seq
        ]
        page = matching_messages[: params.limit]
        anchor_id = params.message_id or path[-1].id
        title = _truncate(
            _single_line(chat.title) if chat.title else "Untitled chat",
            _MAX_CHAT_TITLE,
        )

        if not page:
            return ToolResult(
                content=[
                    {
                        "type": "text",
                        "text": (
                            f'Chat "{_single_line(title)}" has no messages in '
                            f"sequence range {start_seq}-{end_seq}. "
                            f"Branch anchor message_id={anchor_id}."
                        ),
                    }
                ]
            )

        header_lines = [
            "<past-chat-history>",
            f'Chat: "{_single_line(title)}" (chat_id={params.chat_id})',
            f"Branch anchor message_id={anchor_id}",
            "Showing messages in the requested sequence range.",
        ]
        message_lines: list[str] = []
        included_messages: list[ChatMessage] = []
        for message in page:
            role = message.message.get("role")
            role_name = role if isinstance(role, str) else "unknown"
            rendered = _render_message_content(message)
            message_lines.append(
                f"[seq {message.message_seq_num} | {role_name} | id={message.id}]\n{rendered}"
            )
            candidate = "\n\n".join(
                [*header_lines, *message_lines, "</past-chat-history>"]
            )
            if len(candidate) > _MAX_READ_OUTPUT_CHARS and included_messages:
                message_lines.pop()
                break
            included_messages.append(message)

        def update_header() -> None:
            header_lines[3] = (
                f"Showing {len(included_messages)} of {len(matching_messages)} messages "
                f"in the requested range; branch range is {branch_start}-{branch_end}."
            )

        update_header()

        def build_continuation() -> str | None:
            next_index = len(included_messages)
            if next_index >= len(matching_messages):
                return None

            next_start_seq = matching_messages[next_index].message_seq_num
            end_clause = (
                f", end_seq={params.end_seq}" if params.end_seq is not None else ""
            )
            return (
                f"More messages are available (next_start_seq={next_start_seq}). "
                "Call read_chat with "
                f"chat_id={params.chat_id}, message_id={anchor_id}, "
                f"start_seq={next_start_seq}{end_clause}, limit={params.limit}."
            )

        continuation: str | None
        while True:
            continuation = build_continuation()
            candidate_lines = [*header_lines, *message_lines, "</past-chat-history>"]
            if continuation is not None:
                candidate_lines.insert(-1, continuation)
            if (
                len("\n\n".join(candidate_lines)) <= _MAX_READ_OUTPUT_CHARS
                or len(message_lines) <= 1
            ):
                break
            message_lines.pop()
            included_messages.pop()
            update_header()

        output_lines = [*header_lines, *message_lines]
        if continuation is not None:
            output_lines.append(continuation)
        output_lines.append("</past-chat-history>")
        return ToolResult(
            content=[{"type": "text", "text": "\n\n".join(output_lines)}]
        )

    @staticmethod
    def _format_search_hit(hit: ChatSearchHit) -> list[str]:
        title = _truncate(
            _single_line(hit.title) if hit.title else "Untitled chat",
            _MAX_CHAT_TITLE,
        )
        lines = [
            f'- chat_id={hit.chat_id}; title="{title}"; '
            f"updated_at={hit.updated_at.isoformat()}; messages={hit.message_count}; "
            f"matched_in={hit.source}"
        ]
        if hit.snippet:
            snippet = _truncate(_single_line(hit.snippet), _MAX_SEARCH_SNIPPET)
            lines.append(f'  snippet: "{snippet}"')
        if hit.matched_message_id:
            lines.append(
                "  matching_message_id="
                f"{hit.matched_message_id} (pass this as message_id to read_chat for that branch)"
            )
        return lines

    @staticmethod
    def _unavailable_result() -> ToolResult:
        return ToolResult(
            content=[
                {
                    "type": "text",
                    "text": (
                        "Chat history is not available in this session because "
                        "there is no user context."
                    ),
                }
            ]
        )
