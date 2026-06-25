"""
LLM Provider abstraction layer for supporting multiple AI providers.
"""

from abc import ABC, abstractmethod
from collections.abc import AsyncIterator
from dataclasses import dataclass
from typing import Any, ClassVar

from anthropic import MessageStreamEvent

from .types import ProviderError, ProviderType


@dataclass
class TokenUsage:
    input_tokens: int = 0
    output_tokens: int = 0
    cache_read_tokens: int = 0
    cache_creation_tokens: int = 0


class LLMProviderError(Exception):
    """Base exception for provider failures with a user-facing message."""

    def __init__(self, message: str):
        super().__init__(message)
        self.message = message


class LLMProviderStreamError(LLMProviderError):
    """Raised when a provider fails while streaming a response."""


class LLMProviderEmptyResponseError(LLMProviderError):
    """Raised when a provider returns no usable text for a non-streaming response."""


class LLMProvider(ABC):
    """Abstract base class for LLM providers.

    TODO: model_record_id / model_name should not live on the provider —
    a provider represents the connection (endpoint + credentials), not a
    specific model. Move model selection to a request-time parameter on
    the stream/generate calls and let one provider serve many models.
    """

    provider_type: ClassVar[ProviderType]

    # ID of this model's record in the models table
    model_record_id: str | None = None
    model_name: str | None = None
    # Wire-level config exposed for downstream integrations (e.g. mem0
    # which needs to talk to the same endpoint with the same credentials).
    # Subclasses set these in __init__; left None when not applicable.
    api_key: str | None = None
    base_url: str | None = None

    # Provider-specific sidecar fields the provider stamps on streamed
    # content_blocks (via Pydantic ``extra="allow"``) that must be persisted
    # alongside the standard block fields and read back on replay.
    # Example: Gemini's ``_gemini_thought_signature``, an opaque reasoning
    # token Gemini 3 requires to round-trip across turns.
    PERSISTED_BLOCK_EXTRAS: tuple[str, ...] = ()

    @abstractmethod
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
        """Stream a response from the LLM provider. Returns Anthropic MessageStreamEvent objects."""
        pass

    @abstractmethod
    async def generate_response(
        self,
        prompt: str,
        max_tokens: int | None = None,
        temperature: float | None = None,
        top_p: float | None = None,
    ) -> tuple[str, TokenUsage]:
        """Generate a non-streaming response from the LLM provider.

        Returns (response_text, token_usage).
        """
        pass

    @abstractmethod
    async def health_check(self) -> bool:
        """Check if the provider is healthy."""
        pass



# Import all providers after base class definition
from .anthropic import AnthropicProvider
from .openai_compatible import OpenAICompatibleProvider
from .bedrock import BedrockProvider
from .openai import OpenAIProvider
from .gemini import GeminiProvider
from .azure_foundry import AzureFoundryProvider
from .vertex_ai import VertexAIProvider


def create_llm_provider(provider_type: ProviderType | str, **kwargs) -> LLMProvider:
    """Factory function to create LLM provider based on type."""
    pt = ProviderType(provider_type)

    if pt is ProviderType.OPENAI_COMPATIBLE:
        base_url = kwargs.get("base_url")
        if not base_url:
            raise ValueError("base_url is required for OpenAI-compatible provider")
        return OpenAICompatibleProvider(
            base_url,
            api_key=kwargs.get("api_key"),
            model=kwargs.get("model", "default"),
        )

    if pt is ProviderType.ANTHROPIC:
        api_key = kwargs.get("api_key")
        if not api_key:
            raise ValueError("api_key is required for Anthropic provider")
        return AnthropicProvider(
            api_key, kwargs.get("model", "claude-3-5-sonnet-20241022")
        )

    if pt is ProviderType.BEDROCK:
        model_id = (
            kwargs.get("model_id")
            or kwargs.get("model")
            or "us.anthropic.claude-sonnet-4-20250514-v1:0"
        )
        return BedrockProvider(model_id, region_name=kwargs.get("region_name"))

    if pt is ProviderType.OPENAI:
        api_key = kwargs.get("api_key")
        if not api_key:
            raise ValueError("api_key is required for OpenAI provider")
        return OpenAIProvider(api_key, kwargs.get("model", "gpt-4o"))

    if pt is ProviderType.GEMINI:
        api_key = kwargs.get("api_key")
        if not api_key:
            raise ValueError("api_key is required for Gemini provider")
        return GeminiProvider(api_key, kwargs.get("model", "gemini-2.5-flash"))

    if pt is ProviderType.AZURE_FOUNDRY:
        endpoint_url = kwargs.get("endpoint_url")
        if not endpoint_url:
            raise ValueError("endpoint_url is required for Azure AI Foundry provider")
        return AzureFoundryProvider(endpoint_url, kwargs.get("model", "gpt-4o"))

    if pt is ProviderType.VERTEX_AI:
        region = kwargs.get("region")
        project_id = kwargs.get("project_id")
        if not region or not project_id:
            raise ValueError(
                "region and project_id are required for Vertex AI provider"
            )
        return VertexAIProvider(
            region=region,
            project_id=project_id,
            model=kwargs.get("model", "gemini-2.5-flash"),
        )

    raise ValueError(f"Unhandled provider type: {pt!r}")


__all__ = [
    "TokenUsage",
    "LLMProviderError",
    "LLMProviderStreamError",
    "LLMProvider",
    "ProviderType",
    "ProviderError",
    "AnthropicProvider",
    "OpenAICompatibleProvider",
    "BedrockProvider",
    "OpenAIProvider",
    "GeminiProvider",
    "AzureFoundryProvider",
    "VertexAIProvider",
    "create_llm_provider",
]
