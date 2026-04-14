"""Token usage tracking for LLM calls."""

import asyncio
import logging
from collections.abc import AsyncIterator
from dataclasses import dataclass
from enum import StrEnum

from anthropic.types.message_stream_event import MessageStreamEvent

from db.usage import UsageRepository

logger = logging.getLogger(__name__)


class UsagePurpose(StrEnum):
    CHAT = "chat"
    TITLE_GENERATION = "title_generation"
    COMPACTION = "compaction"
    AGENT_RUN = "agent_run"
    AGENT_SUMMARY = "agent_summary"


@dataclass
class UsageContext:
    """Metadata for a usage record — everything except the token counts."""

    user_id: str | None
    model_id: str
    model_name: str
    provider_type: str
    purpose: UsagePurpose
    chat_id: str | None = None
    agent_run_id: str | None = None


class UsageTracker:
    """Wraps an LLM event stream, captures token usage, and persists it fire-and-forget."""

    def __init__(
        self,
        repo: UsageRepository,
        ctx: UsageContext,
    ):
        self._repo = repo
        self._ctx = ctx
        self.input_tokens = 0
        self.output_tokens = 0
        self.cache_read_tokens = 0
        self.cache_creation_tokens = 0

    async def wrap_stream(
        self, stream: AsyncIterator[MessageStreamEvent]
    ) -> AsyncIterator[MessageStreamEvent]:
        async for event in stream:
            if event.type == "message_start":
                usage = event.message.usage
                self.input_tokens = usage.input_tokens
                self.output_tokens = usage.output_tokens
                self.cache_read_tokens = (
                    getattr(usage, "cache_read_input_tokens", 0) or 0
                )
                self.cache_creation_tokens = (
                    getattr(usage, "cache_creation_input_tokens", 0) or 0
                )
            elif event.type == "message_delta":
                usage = getattr(event, "usage", None)
                if usage:
                    # Providers that only learn input_tokens at stream end emit them
                    # on the final message_delta. Overwrite when present; last delta wins.
                    input_tokens = getattr(usage, "input_tokens", None)
                    if input_tokens is not None:
                        self.input_tokens = input_tokens
                    self.output_tokens = usage.output_tokens
                    cache_read = getattr(usage, "cache_read_input_tokens", None)
                    if cache_read is not None:
                        self.cache_read_tokens = cache_read
                    cache_creation = getattr(usage, "cache_creation_input_tokens", None)
                    if cache_creation is not None:
                        self.cache_creation_tokens = cache_creation
            yield event

    def save(self) -> None:
        """Fire-and-forget: persist usage record without blocking the caller."""
        if not (self.input_tokens or self.output_tokens):
            return

        asyncio.create_task(self._persist())

    async def _persist(self) -> None:
        try:
            await self._repo.upsert(
                user_id=self._ctx.user_id,
                model_id=self._ctx.model_id,
                model_name=self._ctx.model_name,
                provider_type=self._ctx.provider_type,
                purpose=self._ctx.purpose,
                input_tokens=self.input_tokens,
                output_tokens=self.output_tokens,
                cache_read_tokens=self.cache_read_tokens,
                cache_creation_tokens=self.cache_creation_tokens,
                chat_id=self._ctx.chat_id,
                agent_run_id=self._ctx.agent_run_id,
            )
        except Exception:
            logger.warning("Failed to persist usage record", exc_info=True)


def track_usage(
    repo: UsageRepository,
    ctx: UsageContext,
    input_tokens: int,
    output_tokens: int,
    cache_read_tokens: int = 0,
    cache_creation_tokens: int = 0,
) -> None:
    """One-shot helper for non-streaming calls where we already have the token counts."""
    if not (input_tokens or output_tokens):
        return

    async def _persist():
        try:
            await repo.upsert(
                user_id=ctx.user_id,
                model_id=ctx.model_id,
                model_name=ctx.model_name,
                provider_type=ctx.provider_type,
                purpose=ctx.purpose,
                input_tokens=input_tokens,
                output_tokens=output_tokens,
                cache_read_tokens=cache_read_tokens,
                cache_creation_tokens=cache_creation_tokens,
                chat_id=ctx.chat_id,
                agent_run_id=ctx.agent_run_id,
            )
        except Exception:
            logger.warning("Failed to persist usage record", exc_info=True)

    asyncio.create_task(_persist())
