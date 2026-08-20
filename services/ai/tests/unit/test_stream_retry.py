"""Unit tests for provider stream recovery in
``streaming.generate.event_stream_with_context_retry``.

Covers the one-shot retry for transport-level failures flagged by the provider
(e.g. an ``httpx.ReadError`` dropping the SSE connection before any content is
sent), the no-retry guards, and the pre-existing context-overflow compaction
retry.
"""

from types import SimpleNamespace
from unittest.mock import AsyncMock, patch

import httpx
import pytest

from providers import ProviderError, ProviderType
from streaming.generate import event_stream_with_context_retry


def _evt(type_: str):
    # Only ``.type`` is inspected by the code under test.
    return SimpleNamespace(type=type_)


class _PassthroughTracker:
    """UsageTracker stand-in that passes events through and counts saves."""

    save_count = 0

    def __init__(self, *args, **kwargs):
        pass

    async def wrap_stream(self, stream):
        async for event in stream:
            yield event

    def save(self):
        _PassthroughTracker.save_count += 1


class _ScriptedProvider:
    """LLMProvider whose stream_response plays a scripted sequence per call."""

    supports_citations = True
    provider_type = ProviderType.OPENAI_COMPATIBLE
    model_record_id = "model-1"
    model_name = "test-model"
    PERSISTED_BLOCK_EXTRAS = ()

    def __init__(self, scripts):
        self._scripts = list(scripts)
        self.calls = 0

    async def stream_response(self, **kwargs):
        script = self._scripts[self.calls]
        self.calls += 1
        async for event in script():
            yield event


def _provider_error(
    message: str,
    status_code: int | None,
    *,
    is_retryable: bool = False,
    is_context_overflow: bool = False,
    cause: BaseException | None = None,
) -> ProviderError:
    return ProviderError(
        message,
        provider_type=ProviderType.OPENAI_COMPATIBLE,
        model="test-model",
        status_code=status_code,
        is_retryable=is_retryable,
        is_context_overflow=is_context_overflow,
        cause=cause,
    )


# --- scripted provider streams --------------------------------------------


async def _drop_mid_stream():
    """Emit the envelope, then the connection dies before any content.

    Shaped like what ``OpenAICompatibleProvider`` actually raises: a
    status-less ProviderError with a transport cause and the retryable flag.
    """
    yield _evt("message_start")
    raise _provider_error(
        "connection lost",
        status_code=None,
        is_retryable=True,
        cause=httpx.ReadError(
            "stream reset", request=httpx.Request("POST", "http://x")
        ),
    )


async def _local_error():
    # No status AND not flagged transport-level (e.g. a local validation
    # ValueError wrapped by the provider). Must not be retried.
    raise _provider_error(
        "invalid tool message sequence",
        status_code=None,
        is_retryable=False,
        cause=ValueError("tool_calls were never answered"),
    )
    yield  # unreachable; marks this as an async generator


async def _rate_limited():
    # Status errors are the provider SDK's retry territory; the outer layer
    # must not retry them again.
    raise _provider_error("rate limited", status_code=429, is_retryable=False)
    yield  # unreachable; marks this as an async generator


async def _context_overflow():
    raise _provider_error("prompt too long", status_code=413, is_context_overflow=True)
    yield  # unreachable; marks this as an async generator


async def _drop_after_content_block_start():
    yield _evt("message_start")
    yield _evt("content_block_start")
    raise _provider_error(
        "connection lost",
        status_code=None,
        is_retryable=True,
        cause=httpx.ReadError(
            "stream reset", request=httpx.Request("POST", "http://x")
        ),
    )


async def _ok_text_stream():
    yield _evt("message_start")
    yield _evt("content_block_start")
    yield _evt("content_block_delta")
    yield _evt("content_block_stop")
    yield _evt("message_stop")


_FULL_OK_TYPES = [
    "message_start",
    "content_block_start",
    "content_block_delta",
    "content_block_stop",
    "message_stop",
]


async def _drain(provider, conversation_messages=None):
    if conversation_messages is None:
        conversation_messages = [{"role": "user", "content": "hi"}]
    compactor = SimpleNamespace(
        select_legacy_compaction_split=lambda msgs: None,
        compact_conversation=AsyncMock(
            return_value=[{"role": "user", "content": "compacted"}]
        ),
    )
    sleep_mock = AsyncMock()
    _PassthroughTracker.save_count = 0
    events = []
    with (
        patch("streaming.generate.UsageTracker", _PassthroughTracker),
        patch("streaming.generate.UsageRepository", lambda: None),
        patch("asyncio.sleep", sleep_mock),
    ):
        stream = event_stream_with_context_retry(
            turn_tools=[],
            conversation_messages=conversation_messages,
            llm_provider=provider,
            chat_user_id="user-1",
            chat_id="chat-1",
            system_prompt="sys",
            compactor=compactor,
            latest_compaction_summary=None,
            summarizer_context_window_tokens=100_000,
        )
        async for event in stream:
            events.append(event)
    return events, conversation_messages, compactor, sleep_mock


@pytest.mark.unit
class TestTransientStreamRetry:
    @pytest.mark.asyncio
    async def test_mid_stream_drop_retries_once_and_recovers(self):
        provider = _ScriptedProvider([_drop_mid_stream, _ok_text_stream])
        events, _, _, sleep_mock = await _drain(provider)

        assert provider.calls == 2
        types = [e.type for e in events]
        # The original envelope is retained, the retried stream's duplicate
        # envelope is dropped, and the recovered content follows it. Exactly
        # one message_start reaches the caller, so persistence creates exactly
        # one assistant row.
        assert types.count("message_start") == 1
        assert types == _FULL_OK_TYPES
        # One backoff sleep before the retry, and the failed attempt's usage
        # was persisted (both attempts count toward the upsert).
        sleep_mock.assert_awaited_once_with(1.0)
        assert _PassthroughTracker.save_count == 2

    @pytest.mark.asyncio
    async def test_non_retryable_statusless_error_is_not_retried(self):
        # No HTTP status alone does not make an error transient: a local
        # validation error wrapped by the provider must not be replayed.
        provider = _ScriptedProvider([_local_error])
        with pytest.raises(ProviderError) as exc:
            await _drain(provider)

        assert provider.calls == 1
        assert exc.value.status_code is None
        assert exc.value.is_retryable is False

    @pytest.mark.asyncio
    async def test_status_error_not_retried_by_outer_layer(self):
        # 429/5xx are retried by the provider SDK at request-creation time;
        # retrying them here too would multiply the request count.
        provider = _ScriptedProvider([_rate_limited])
        with pytest.raises(ProviderError) as exc:
            await _drain(provider)

        assert provider.calls == 1
        assert exc.value.status_code == 429

    @pytest.mark.asyncio
    async def test_no_retry_after_content_block_started(self):
        provider = _ScriptedProvider([_drop_after_content_block_start])
        with pytest.raises(ProviderError):
            await _drain(provider)

        # Once a content block has started, the run cannot be cleanly redone.
        assert provider.calls == 1

    @pytest.mark.asyncio
    async def test_second_failure_is_not_retried_again(self):
        provider = _ScriptedProvider([_drop_mid_stream, _drop_mid_stream])
        with pytest.raises(ProviderError):
            await _drain(provider)

        assert provider.calls == 2

    @pytest.mark.asyncio
    async def test_context_overflow_still_compacts_and_retries(self):
        provider = _ScriptedProvider([_context_overflow, _ok_text_stream])
        conversation_messages = [{"role": "user", "content": "hi"}]
        events, conversation_messages, compactor, _ = await _drain(
            provider, conversation_messages
        )

        assert provider.calls == 2
        assert compactor.compact_conversation.await_count == 1
        assert conversation_messages == [{"role": "user", "content": "compacted"}]
        # Attempt one failed at request time, so attempt two's envelope is kept.
        assert [e.type for e in events] == _FULL_OK_TYPES
