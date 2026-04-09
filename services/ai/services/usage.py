"""Token usage tracking for LLM calls."""

import asyncio
import logging
from collections.abc import AsyncIterator
from dataclasses import dataclass

from anthropic.types.message_stream_event import MessageStreamEvent

from db.usage import UsageRepository
from providers import LLMProvider

logger = logging.getLogger(__name__)


@dataclass
class UsageContext:
    """Metadata for a usage record — everything except the token counts."""

    user_id: str | None
    model_id: str
    model_name: str
    provider_type: str
    usage_type: str
    chat_id: str | None = None
    agent_run_id: str | None = None


class UsageTracker:
    """Wraps an LLM event stream, captures token usage, and persists it fire-and-forget."""

    def __init__(
        self,
        repo: UsageRepository,
        ctx: UsageContext,
        provider: LLMProvider | None = None,
    ):
        self._repo = repo
        self._ctx = ctx
        self._provider = provider
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
                if hasattr(event, "usage") and event.usage:
                    self.output_tokens = event.usage.output_tokens
            yield event

    def save(self) -> None:
        """Fire-and-forget: persist usage record without blocking the caller."""
        # Some providers (Gemini, vLLM) set last_usage on the provider instance
        # with data not available in streaming events (e.g., input_tokens).
        if self._provider and self._provider.last_usage:
            pu = self._provider.last_usage
            if not self.input_tokens:
                self.input_tokens = pu.input_tokens
            if not self.output_tokens:
                self.output_tokens = pu.output_tokens
            if not self.cache_read_tokens:
                self.cache_read_tokens = pu.cache_read_tokens
            if not self.cache_creation_tokens:
                self.cache_creation_tokens = pu.cache_creation_tokens

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
                usage_type=self._ctx.usage_type,
                input_tokens=self.input_tokens,
                output_tokens=self.output_tokens,
                cache_read_tokens=self.cache_read_tokens,
                cache_creation_tokens=self.cache_creation_tokens,
                chat_id=self._ctx.chat_id,
                agent_run_id=self._ctx.agent_run_id,
            )
        except Exception:
            logger.warning("Failed to persist usage record", exc_info=True)


def save_usage_fire_and_forget(
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
                usage_type=ctx.usage_type,
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
