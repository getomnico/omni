"""
Google Cloud Vertex AI Provider.

Supports both Anthropic (Claude) and Gemini models on Vertex AI.
Auto-detects model family from model ID and delegates to the appropriate provider.
Uses Application Default Credentials (ADC) for authentication — no API key needed.
"""

import logging
from collections.abc import AsyncIterator
from typing import Any

from anthropic import AsyncAnthropicVertex, MessageStreamEvent
from google import genai

from . import LLMProvider
from .anthropic import AnthropicProvider
from .gemini import GeminiProvider

logger = logging.getLogger(__name__)


def _is_claude_model(model: str) -> bool:
    """Detect if a model ID refers to an Anthropic model."""
    lower = model.lower()
    return "claude" in lower or "anthropic" in lower


class VertexAIProvider(LLMProvider):
    """Provider for models on Google Cloud Vertex AI.

    Auto-detects whether to use the Anthropic or Gemini SDK based on the model ID,
    and authenticates via Application Default Credentials (ADC).
    """

    def __init__(self, region: str, project_id: str, model: str):
        self.region = region
        self.project_id = project_id
        self.model = model

        if _is_claude_model(model):
            client = AsyncAnthropicVertex(region=region, project_id=project_id)
            self._delegate = AnthropicProvider(api_key="unused", model=model)
            self._delegate.client = client
        else:
            client = genai.Client(vertexai=True, project=project_id, location=region)
            self._delegate = GeminiProvider(api_key="unused", model=model)
            self._delegate.client = client

        logger.info(
            f"Initialized VertexAIProvider for model '{model}' "
            f"(family={'anthropic' if _is_claude_model(model) else 'gemini'}, "
            f"region={region}, project={project_id})"
        )

    async def stream_response(
        self,
        prompt: str,
        max_tokens: int | None = None,
        temperature: float | None = None,
        top_p: float | None = None,
        tools: list[dict[str, Any]] | None = None,
        messages: list[dict[str, Any]] | None = None,
        system_prompt: str | None = None,
    ) -> AsyncIterator[MessageStreamEvent]:
        async for event in self._delegate.stream_response(
            prompt=prompt,
            max_tokens=max_tokens,
            temperature=temperature,
            top_p=top_p,
            tools=tools,
            messages=messages,
            system_prompt=system_prompt,
        ):
            yield event
        self.last_usage = self._delegate.last_usage

    async def generate_response(
        self,
        prompt: str,
        max_tokens: int | None = None,
        temperature: float | None = None,
        top_p: float | None = None,
    ) -> str:
        result = await self._delegate.generate_response(
            prompt=prompt,
            max_tokens=max_tokens,
            temperature=temperature,
            top_p=top_p,
        )
        self.last_usage = self._delegate.last_usage
        return result

    async def health_check(self) -> bool:
        return await self._delegate.health_check()
