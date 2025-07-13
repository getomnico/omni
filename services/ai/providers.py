"""
LLM Provider abstraction layer for supporting multiple AI providers.
"""

import asyncio
import json
import os
from abc import ABC, abstractmethod
from typing import AsyncIterator, Optional
import logging

import httpx
from anthropic import AsyncAnthropic
from anthropic.types import ContentBlockDeltaEvent, ContentBlockStartEvent

logger = logging.getLogger(__name__)


class LLMProvider(ABC):
    """Abstract base class for LLM providers."""

    @abstractmethod
    async def stream_response(
        self,
        prompt: str,
        max_tokens: Optional[int] = None,
        temperature: Optional[float] = None,
        top_p: Optional[float] = None,
    ) -> AsyncIterator[str]:
        """Stream a response from the LLM provider."""
        pass

    @abstractmethod
    async def generate_response(
        self,
        prompt: str,
        max_tokens: Optional[int] = None,
        temperature: Optional[float] = None,
        top_p: Optional[float] = None,
    ) -> str:
        """Generate a non-streaming response from the LLM provider."""
        pass

    @abstractmethod
    async def health_check(self) -> bool:
        """Check if the provider is healthy."""
        pass


class VLLMProvider(LLMProvider):
    """Provider for vLLM OpenAI-compatible API."""

    def __init__(self, vllm_url: str):
        self.vllm_url = vllm_url
        self.client = httpx.AsyncClient(timeout=60.0)

    async def stream_response(
        self,
        prompt: str,
        max_tokens: Optional[int] = None,
        temperature: Optional[float] = None,
        top_p: Optional[float] = None,
    ) -> AsyncIterator[str]:
        """Stream response from vLLM service."""
        payload = {
            "model": "placeholder",  # vLLM ignores this but requires it
            "messages": [{"role": "user", "content": prompt}],
            "max_tokens": max_tokens or 512,
            "temperature": temperature or 0.7,
            "top_p": top_p or 0.9,
            "stream": True,
        }

        try:
            async with self.client.stream(
                "POST",
                f"{self.vllm_url}/v1/chat/completions",
                json=payload,
                headers={"Accept": "text/event-stream"},
            ) as response:
                response.raise_for_status()

                async for chunk in response.aiter_lines():
                    if chunk:
                        if chunk.startswith("data: "):
                            chunk_data = chunk[6:]  # Remove "data: " prefix

                            if chunk_data == "[DONE]":
                                break

                            try:
                                chunk_json = json.loads(chunk_data)
                                choices = chunk_json.get("choices", [])
                                if choices and len(choices) > 0:
                                    delta = choices[0].get("delta", {})
                                    content = delta.get("content", "")
                                    if content:
                                        yield content
                            except json.JSONDecodeError:
                                continue

        except httpx.TimeoutException:
            logger.error("Timeout while calling vLLM service")
            yield "Error: Request timeout"
        except httpx.HTTPStatusError as e:
            logger.error(f"HTTP error from vLLM service: {e.response.status_code}")
            yield f"Error: vLLM service error ({e.response.status_code})"
        except Exception as e:
            logger.error(f"Failed to stream from vLLM: {str(e)}")
            yield f"Error: {str(e)}"

    async def generate_response(
        self,
        prompt: str,
        max_tokens: Optional[int] = None,
        temperature: Optional[float] = None,
        top_p: Optional[float] = None,
    ) -> str:
        """Generate non-streaming response from vLLM service."""
        payload = {
            "model": "placeholder",  # vLLM ignores this but requires it
            "messages": [{"role": "user", "content": prompt}],
            "max_tokens": max_tokens or 512,
            "temperature": temperature or 0.7,
            "top_p": top_p or 0.9,
            "stream": False,
        }

        try:
            response = await self.client.post(
                f"{self.vllm_url}/v1/chat/completions", json=payload
            )
            response.raise_for_status()

            vllm_response = response.json()
            choices = vllm_response.get("choices", [])
            if not choices:
                raise Exception("No choices in vLLM response")

            message = choices[0].get("message", {})
            generated_text = message.get("content", "")

            if not generated_text:
                raise Exception("Empty response from vLLM service")

            return generated_text

        except httpx.TimeoutException:
            raise Exception("Request to vLLM service timed out")
        except httpx.HTTPStatusError as e:
            raise Exception(f"vLLM service error: {e.response.status_code}")
        except Exception as e:
            raise Exception(f"Failed to generate response: {str(e)}")

    async def health_check(self) -> bool:
        """Check if vLLM service is healthy."""
        try:
            response = await self.client.get(f"{self.vllm_url}/health")
            return response.status_code == 200
        except Exception:
            return False


class AnthropicProvider(LLMProvider):
    """Provider for Anthropic Claude API."""

    def __init__(self, api_key: str, model: str = "claude-3-5-sonnet-20241022"):
        self.client = AsyncAnthropic(api_key=api_key)
        self.model = model

    async def stream_response(
        self,
        prompt: str,
        max_tokens: Optional[int] = None,
        temperature: Optional[float] = None,
        top_p: Optional[float] = None,
    ) -> AsyncIterator[str]:
        """Stream response from Anthropic Claude API."""
        try:
            stream = await self.client.messages.create(
                model=self.model,
                messages=[{"role": "user", "content": prompt}],
                max_tokens=max_tokens or 4096,
                temperature=temperature or 0.7,
                top_p=top_p or 0.9,
                stream=True,
            )

            async for event in stream:
                if isinstance(event, ContentBlockDeltaEvent):
                    yield event.delta.text
                elif isinstance(event, ContentBlockStartEvent):
                    # For text blocks, we don't need to yield anything here
                    # The actual content comes in ContentBlockDeltaEvent
                    pass

        except Exception as e:
            logger.error(f"Failed to stream from Anthropic: {str(e)}")
            yield f"Error: {str(e)}"

    async def generate_response(
        self,
        prompt: str,
        max_tokens: Optional[int] = None,
        temperature: Optional[float] = None,
        top_p: Optional[float] = None,
    ) -> str:
        """Generate non-streaming response from Anthropic Claude API."""
        try:
            response = await self.client.messages.create(
                model=self.model,
                messages=[{"role": "user", "content": prompt}],
                max_tokens=max_tokens or 4096,
                temperature=temperature or 0.7,
                top_p=top_p or 0.9,
                stream=False,
            )

            # Extract text content from response
            content = ""
            for block in response.content:
                if hasattr(block, "text"):
                    content += block.text

            return content

        except Exception as e:
            logger.error(f"Failed to generate response from Anthropic: {str(e)}")
            raise Exception(f"Failed to generate response: {str(e)}")

    async def health_check(self) -> bool:
        """Check if Anthropic API is accessible."""
        try:
            # Try a minimal request to check API accessibility
            response = await self.client.messages.create(
                model=self.model,
                messages=[{"role": "user", "content": "Hello"}],
                max_tokens=1,
                stream=False,
            )
            return True
        except Exception:
            return False


def create_llm_provider(provider_type: str, **kwargs) -> LLMProvider:
    """Factory function to create LLM provider based on type."""
    if provider_type.lower() == "vllm":
        vllm_url = kwargs.get("vllm_url")
        if not vllm_url:
            raise ValueError("vllm_url is required for vLLM provider")
        return VLLMProvider(vllm_url)

    elif provider_type.lower() == "anthropic":
        api_key = kwargs.get("api_key")
        if not api_key:
            raise ValueError("api_key is required for Anthropic provider")
        model = kwargs.get("model", "claude-3-5-sonnet-20241022")
        return AnthropicProvider(api_key, model)

    else:
        raise ValueError(f"Unknown provider type: {provider_type}")
