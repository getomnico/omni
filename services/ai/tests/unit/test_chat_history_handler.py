from datetime import UTC, datetime
from typing import Any

import pytest

from db.models import Chat, ChatMessage, ChatSearchHit
from tools.chat_history_handler import (
    _MAX_MESSAGE_TEXT,
    ChatHistoryToolHandler,
    _render_message_content,
    _truncate,
)
from tools.registry import ToolContext


class FakeChatsRepository:
    def __init__(self, hits: list[ChatSearchHit], chat: Chat | None = None) -> None:
        self.hits = hits
        self.chat = chat
        self.search_args: dict[str, Any] | None = None

    async def search(self, **kwargs: Any) -> list[ChatSearchHit]:
        self.search_args = kwargs
        return self.hits

    async def get(self, chat_id: str) -> Chat | None:
        return self.chat if self.chat is not None and self.chat.id == chat_id else None


class FakeMessagesRepository:
    def __init__(self, paths: dict[str | None, list[ChatMessage]]) -> None:
        self.paths = paths
        self.path_args: tuple[str, str | None] | None = None

    async def get_active_path(
        self, chat_id: str, message_id: str | None = None
    ) -> list[ChatMessage]:
        self.path_args = (chat_id, message_id)
        return self.paths.get(message_id, [])


def _chat() -> Chat:
    now = datetime.now(UTC)
    return Chat(
        id="01CHAT",
        user_id="01USER",
        title="Planning conversation",
        model_id=None,
        created_at=now,
        updated_at=now,
    )


def _message(
    message_id: str,
    sequence: int,
    content: object,
    parent_id: str | None = None,
) -> ChatMessage:
    return ChatMessage(
        id=message_id,
        chat_id="01CHAT",
        message_seq_num=sequence,
        message={"role": "user" if sequence % 2 else "assistant", "content": content},
        created_at=datetime.now(UTC),
        parent_id=parent_id,
    )


def _context(user_id: str | None = "01USER") -> ToolContext:
    return ToolContext(chat_id="01CURRENT", user_id=user_id)


@pytest.mark.asyncio
async def test_search_chats_is_user_scoped_and_returns_branch_anchor() -> None:
    hits = [
        ChatSearchHit(
            chat_id="01CHAT",
            title="Planning conversation",
            updated_at=datetime(2026, 1, 1, tzinfo=UTC),
            message_count=4,
            snippet="We decided to ship the feature in March.",
            source="message",
            matched_message_id="01MATCH",
        )
    ]
    chats = FakeChatsRepository(hits)
    handler = ChatHistoryToolHandler(
        chats_repo=chats, messages_repo=FakeMessagesRepository({})
    )

    result = await handler.execute(
        "search_chats", {"query": "feature launch"}, _context()
    )

    assert result.is_error is False
    text = result.content[0]["text"]
    assert "01CHAT" in text
    assert "01MATCH" in text
    assert chats.search_args == {
        "user_id": "01USER",
        "query": "feature launch",
        "limit": 10,
        "exclude_chat_id": "01CURRENT",
    }


@pytest.mark.asyncio
async def test_read_chat_paginates_active_branch() -> None:
    chat = _chat()
    first = _message("01ROOT", 1, "What did we decide?")
    second = _message("01ASSIST", 2, "We decided to ship in March.", parent_id=first.id)
    third = _message("01LAST", 3, "Thanks.", parent_id=second.id)
    path = [first, second, third]
    messages = FakeMessagesRepository({None: path})
    handler = ChatHistoryToolHandler(
        chats_repo=FakeChatsRepository([], chat), messages_repo=messages
    )

    result = await handler.execute(
        "read_chat", {"chat_id": chat.id, "limit": 2}, _context()
    )

    assert result.is_error is False
    text = result.content[0]["text"]
    assert "seq 1" in text
    assert "seq 2" in text
    assert "seq 3" not in text
    assert "start_seq=3" in text
    assert "message_id=01LAST" in text
    assert messages.path_args == (chat.id, None)


@pytest.mark.asyncio
async def test_read_chat_uses_requested_branch_and_sequence_range() -> None:
    chat = _chat()
    root = _message("01ROOT", 1, "Root")
    branch_message = _message("01BRANCH", 4, "Edited answer", parent_id=root.id)
    branch = [root, branch_message]
    messages = FakeMessagesRepository({branch_message.id: branch})
    handler = ChatHistoryToolHandler(
        chats_repo=FakeChatsRepository([], chat), messages_repo=messages
    )

    result = await handler.execute(
        "read_chat",
        {
            "chat_id": chat.id,
            "message_id": branch_message.id,
            "start_seq": 4,
            "end_seq": 4,
        },
        _context(),
    )

    text = result.content[0]["text"]
    assert "Edited answer" in text
    assert "Root" not in text
    assert messages.path_args == (chat.id, branch_message.id)


@pytest.mark.asyncio
async def test_chat_history_is_unavailable_without_user_context() -> None:
    chats = FakeChatsRepository([])
    handler = ChatHistoryToolHandler(
        chats_repo=chats, messages_repo=FakeMessagesRepository({})
    )

    result = await handler.execute("search_chats", {"query": "anything"}, _context(None))

    assert result.is_error is False
    assert "no user context" in result.content[0]["text"]
    assert chats.search_args is None


@pytest.mark.asyncio
async def test_read_chat_summarizes_tool_blocks_and_omits_thinking() -> None:
    chat = _chat()
    message = _message(
        "01MSG",
        1,
        [
            {"type": "thinking", "thinking": "secret reasoning"},
            {"type": "tool_use", "name": "search", "input": {"query": "launch"}},
            {
                "type": "tool_result",
                "content": [{"type": "text", "text": "Found the launch decision."}],
            },
        ],
    )
    handler = ChatHistoryToolHandler(
        chats_repo=FakeChatsRepository([], chat),
        messages_repo=FakeMessagesRepository({None: [message]}),
    )

    result = await handler.execute("read_chat", {"chat_id": chat.id}, _context())

    text = result.content[0]["text"]
    assert "Tool call: search" in text
    assert "Found the launch decision." in text
    assert "secret reasoning" not in text


@pytest.mark.asyncio
async def test_read_chat_preserves_end_sequence_in_pagination() -> None:
    chat = _chat()
    path = [_message(f"01MSG{index}", index, f"Message {index}") for index in range(1, 5)]
    handler = ChatHistoryToolHandler(
        chats_repo=FakeChatsRepository([], chat),
        messages_repo=FakeMessagesRepository({None: path}),
    )

    result = await handler.execute(
        "read_chat",
        {"chat_id": chat.id, "limit": 2, "end_seq": 3},
        _context(),
    )

    text = result.content[0]["text"]
    assert "seq 1" in text
    assert "seq 2" in text
    assert "seq 3" not in text
    assert "seq 4" not in text
    assert "start_seq=3, end_seq=3" in text


def test_render_message_content_truncates_combined_text_blocks_once() -> None:
    content = [
        {"type": "text", "text": "a" * 3_000},
        {"type": "text", "text": "b" * 3_000},
    ]
    rendered = _render_message_content(_message("01LONG", 1, content))

    expected = _truncate("a" * 3_000 + "\n" + "b" * 3_000, _MAX_MESSAGE_TEXT)
    assert rendered == expected
    assert len(rendered) <= _MAX_MESSAGE_TEXT


@pytest.mark.asyncio
async def test_read_chat_recounts_messages_after_output_trimming() -> None:
    chat = _chat()
    path = [_message(f"01MSG{index}", index, "a" * 622) for index in range(1, 51)]
    handler = ChatHistoryToolHandler(
        chats_repo=FakeChatsRepository([], chat),
        messages_repo=FakeMessagesRepository({None: path}),
    )

    result = await handler.execute(
        "read_chat", {"chat_id": chat.id, "limit": 50}, _context()
    )

    text = result.content[0]["text"]
    rendered_count = sum(1 for line in text.splitlines() if line.startswith("[seq "))
    assert f"Showing {rendered_count} of 50 messages" in text


@pytest.mark.asyncio
async def test_read_chat_caps_total_output_and_continues() -> None:
    chat = _chat()
    large_content = [
        {"type": "text", "text": "a" * 1_500},
        {"type": "text", "text": "b" * 1_500},
    ]
    path = [_message(f"01MSG{index}", index, large_content) for index in range(1, 21)]
    handler = ChatHistoryToolHandler(
        chats_repo=FakeChatsRepository([], chat),
        messages_repo=FakeMessagesRepository({None: path}),
    )

    result = await handler.execute("read_chat", {"chat_id": chat.id}, _context())

    text = result.content[0]["text"]
    assert len(text) <= 32_000
    assert "next_start_seq=" in text
