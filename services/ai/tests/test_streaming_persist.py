import json
from types import SimpleNamespace

import pytest

from streaming.persist import EndOfStreamReason, end_of_stream, persist_and_transform


class EmptyMessagesRepository:
    def __init__(self) -> None:
        self.created: list[dict[str, object]] = []

    async def create(
        self,
        chat_id: str,
        message: dict[str, object],
        parent_id: str | None = None,
    ) -> SimpleNamespace:
        self.created.append(message)
        return SimpleNamespace(id="01ARZ3NDEKTSV4RRFFQ69G5FAV")


class FakeRedis:
    def __init__(self) -> None:
        self.values: dict[str, tuple[str, int]] = {}

    async def set(self, key: str, value: str, *, ex: int) -> None:
        self.values[key] = (value, ex)


@pytest.mark.asyncio
async def test_empty_limited_turn_emits_valid_message_id_sse() -> None:
    async def generator():
        yield end_of_stream(EndOfStreamReason.ITERATION_LIMIT)

    repository = EmptyMessagesRepository()
    redis = FakeRedis()
    events = [
        event
        async for event in persist_and_transform(
            generator(), "chat-id", repository, None, redis_client=redis
        )
    ]

    assert events[0] == "event: message_id\ndata: 01ARZ3NDEKTSV4RRFFQ69G5FAV\n\n"
    assert r"\n" not in events[0]
    terminal_payload = json.loads(events[1].split("data: ", 1)[1])
    assert terminal_payload["reason"] == "iteration_limit"
    assert terminal_payload["message_id"] == "01ARZ3NDEKTSV4RRFFQ69G5FAV"
    assert repository.created == [{"role": "assistant", "content": []}]
    assert redis.values["chat:iteration-limit:chat-id"][0] == "01ARZ3NDEKTSV4RRFFQ69G5FAV"
