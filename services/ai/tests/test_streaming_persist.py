from types import SimpleNamespace

import pytest

from streaming.persist import EndOfStreamReason, end_of_stream, persist_and_transform


class EmptyMessagesRepository:
    def __init__(self) -> None:
        self.created: list[dict[str, object]] = []
        self.terminal_updates: list[tuple[str, str]] = []

    async def create(
        self,
        chat_id: str,
        message: dict[str, object],
        parent_id: str | None = None,
    ) -> SimpleNamespace:
        self.created.append(message)
        return SimpleNamespace(id="01ARZ3NDEKTSV4RRFFQ69G5FAV")

    async def update_terminal_reason(self, message_id: str, reason: str) -> None:
        self.terminal_updates.append((message_id, reason))


@pytest.mark.asyncio
async def test_empty_limited_turn_emits_valid_message_id_sse() -> None:
    async def generator():
        yield end_of_stream(EndOfStreamReason.ITERATION_LIMIT)

    repository = EmptyMessagesRepository()
    events = [
        event
        async for event in persist_and_transform(
            generator(), "chat-id", repository, None
        )
    ]

    assert events[0] == "event: message_id\ndata: 01ARZ3NDEKTSV4RRFFQ69G5FAV\n\n"
    assert events[1] == end_of_stream(EndOfStreamReason.ITERATION_LIMIT)
    assert repository.created == [{"role": "assistant", "content": []}]
    assert repository.terminal_updates == [
        ("01ARZ3NDEKTSV4RRFFQ69G5FAV", "iteration_limit")
    ]
