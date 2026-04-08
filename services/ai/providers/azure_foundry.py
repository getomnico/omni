"""
Azure AI Foundry Provider.

Supports both Anthropic (Claude) and OpenAI (GPT) models deployed on Azure AI Foundry.
Auto-detects model family from model ID and delegates to the appropriate provider.
Uses DefaultAzureCredential for authentication (Managed Identity on Azure VMs).

For OpenAI models, uses the v1 API (no dated api-version required):
  https://learn.microsoft.com/en-us/azure/foundry/openai/api-version-lifecycle
"""

import asyncio
import logging
from collections.abc import AsyncIterator
from typing import Any

from anthropic import AsyncAnthropicFoundry, MessageStreamEvent
from azure.identity import DefaultAzureCredential, get_bearer_token_provider
from openai import AsyncOpenAI

from . import LLMProvider
from .anthropic import AnthropicProvider
from .openai import OpenAIProvider

logger = logging.getLogger(__name__)

ANTHROPIC_TOKEN_SCOPE = "https://cognitiveservices.azure.com/.default"
OPENAI_TOKEN_SCOPE = "https://ai.azure.com/.default"


def _is_anthropic_model(model: str) -> bool:
    """Detect if a model ID refers to an Anthropic model."""
    lower = model.lower()
    return "claude" in lower or "anthropic" in lower


class AzureFoundryProvider(LLMProvider):
    """Provider for models deployed on Azure AI Foundry.

    Auto-detects whether to use the Anthropic or OpenAI SDK based on the model ID,
    and authenticates via Azure Managed Identity (DefaultAzureCredential).
    """

    def __init__(self, endpoint_url: str, model: str):
        self.endpoint_url = endpoint_url.rstrip("/")
        self.model = model

        credential = DefaultAzureCredential()

        if _is_anthropic_model(model):
            token_provider = get_bearer_token_provider(
                credential, ANTHROPIC_TOKEN_SCOPE
            )
            client = AsyncAnthropicFoundry(
                azure_ad_token_provider=token_provider,
                base_url=f"{self.endpoint_url}/anthropic/",
            )
            self._delegate = AnthropicProvider(api_key="unused", model=model)
            self._delegate.client = client
        else:
            sync_token_provider = get_bearer_token_provider(
                credential, OPENAI_TOKEN_SCOPE
            )

            async def _async_token_provider() -> str:
                return await asyncio.to_thread(sync_token_provider)

            client = AsyncOpenAI(
                api_key=_async_token_provider,
                base_url=f"{self.endpoint_url}/openai/v1/",
            )
            self._delegate = OpenAIProvider(api_key="unused", model=model)
            self._delegate.client = client

        logger.info(
            f"Initialized AzureFoundryProvider for model '{model}' "
            f"(family={'anthropic' if _is_anthropic_model(model) else 'openai'}, "
            f"endpoint={self.endpoint_url})"
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
