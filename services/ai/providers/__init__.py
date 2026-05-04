"""
LLM Provider abstraction layer for supporting multiple AI providers.
"""

from abc import ABC, abstractmethod
from collections.abc import AsyncIterator
from dataclasses import dataclass
from typing import Any

from anthropic import MessageStreamEvent


@dataclass
class TokenUsage:
    input_tokens: int = 0
    output_tokens: int = 0
    cache_read_tokens: int = 0
    cache_creation_tokens: int = 0


class LLMProvider(ABC):
    """Abstract base class for LLM providers.

    TODO: model_record_id / model_name should not live on the provider —
    a provider represents the connection (endpoint + credentials), not a
    specific model. Move model selection to a request-time parameter on
    the stream/generate calls and let one provider serve many models.
    """

    # ID of this model's record in the models table
    model_record_id: str | None = None
    model_name: str | None = None
    provider_type: str | None = None
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


# Factory function to create LLM providers
def create_llm_provider(provider_type: str, **kwargs) -> LLMProvider:
    """Factory function to create LLM provider based on type."""
    if provider_type.lower() == "openai_compatible":
        base_url = kwargs.get("base_url")
        if not base_url:
            raise ValueError("base_url is required for OpenAI-compatible provider")
        api_key = kwargs.get("api_key")
        model = kwargs.get("model", "default")
        return OpenAICompatibleProvider(base_url, api_key=api_key, model=model)

    elif provider_type.lower() == "anthropic":
        api_key = kwargs.get("api_key")
        if not api_key:
            raise ValueError("api_key is required for Anthropic provider")
        model = kwargs.get("model", "claude-3-5-sonnet-20241022")
        return AnthropicProvider(api_key, model)

    elif provider_type.lower() == "bedrock":
        model_id = kwargs.get("model_id", "us.anthropic.claude-sonnet-4-20250514-v1:0")
        region_name = kwargs.get("region_name")
        return BedrockProvider(model_id, region_name=region_name)

    elif provider_type.lower() == "openai":
        api_key = kwargs.get("api_key")
        if not api_key:
            raise ValueError("api_key is required for OpenAI provider")
        model = kwargs.get("model", "gpt-4o")
        return OpenAIProvider(api_key, model)

    elif provider_type.lower() == "gemini":
        api_key = kwargs.get("api_key")
        if not api_key:
            raise ValueError("api_key is required for Gemini provider")
        model = kwargs.get("model", "gemini-2.5-flash")
        return GeminiProvider(api_key, model)

    elif provider_type.lower() == "azure_foundry":
        endpoint_url = kwargs.get("endpoint_url")
        if not endpoint_url:
            raise ValueError("endpoint_url is required for Azure AI Foundry provider")
        model = kwargs.get("model", "gpt-4o")
        return AzureFoundryProvider(endpoint_url, model)

    elif provider_type.lower() == "vertex_ai":
        region = kwargs.get("region")
        project_id = kwargs.get("project_id")
        if not region or not project_id:
            raise ValueError(
                "region and project_id are required for Vertex AI provider"
            )
        model = kwargs.get("model", "gemini-2.5-flash")
        return VertexAIProvider(region=region, project_id=project_id, model=model)

    else:
        raise ValueError(f"Unknown provider type: {provider_type}")


__all__ = [
    "TokenUsage",
    "LLMProvider",
    "AnthropicProvider",
    "OpenAICompatibleProvider",
    "BedrockProvider",
    "OpenAIProvider",
    "GeminiProvider",
    "AzureFoundryProvider",
    "VertexAIProvider",
    "create_llm_provider",
]
