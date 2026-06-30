"""Unit tests for ProviderError wrapping in LLM providers.

Each provider's generate_response and stream_response should wrap underlying SDK
exceptions in ProviderError with the original message, provider name, and (where
the SDK exposes it) HTTP status code preserved.
"""

from unittest.mock import AsyncMock, MagicMock, patch

import httpx
import pytest

from providers import ProviderError, ProviderType


def _anthropic_status_error(message: str, status_code: int):
    """Build a real anthropic.APIStatusError so isinstance checks work."""
    from anthropic import APIStatusError

    request = httpx.Request("POST", "https://api.anthropic.com/v1/messages")
    response = httpx.Response(status_code, request=request, json={"error": message})
    return APIStatusError(message, response=response, body=None)


def _openai_status_error(message: str, status_code: int):
    from openai import APIStatusError

    request = httpx.Request("POST", "https://api.openai.com/v1/responses")
    response = httpx.Response(status_code, request=request, json={"error": message})
    return APIStatusError(message, response=response, body=None)


def _gemini_api_error(message: str, status_code: int):
    from google.genai.errors import APIError

    return APIError(status_code, {"error": {"message": message}})


@pytest.mark.unit
class TestAnthropicProviderError:
    @pytest.fixture
    def provider(self):
        from providers.anthropic import AnthropicProvider

        return AnthropicProvider(api_key="test", model="claude-3-5-sonnet-20241022")

    @pytest.mark.asyncio
    async def test_generate_response_wraps_exception(self, provider):
        provider.client.messages.create = AsyncMock(
            side_effect=_anthropic_status_error(
                "use case details have not been submitted", 404
            )
        )

        with pytest.raises(ProviderError) as exc:
            await provider.generate_response("hello")

        assert "use case details" in exc.value.message
        assert exc.value.provider_type is ProviderType.ANTHROPIC
        assert exc.value.status_code == 404
        assert exc.value.model == "claude-3-5-sonnet-20241022"

    @pytest.mark.asyncio
    async def test_stream_response_wraps_exception(self, provider):
        provider.client.messages.create = AsyncMock(
            side_effect=_anthropic_status_error("invalid api key", 401)
        )

        with pytest.raises(ProviderError) as exc:
            async for _ in provider.stream_response("hello"):
                pass

        assert "invalid api key" in exc.value.message
        assert exc.value.provider_type is ProviderType.ANTHROPIC
        assert exc.value.status_code == 401

    @pytest.mark.asyncio
    async def test_stream_response_does_not_swallow_errors(self, provider):
        """Regression: previously stream_response only logged errors, never
        re-raised — silently truncating responses. It must re-raise now."""
        provider.client.messages.create = AsyncMock(
            side_effect=Exception("transport failure")
        )

        # If the generator silently terminates we'd never enter the except.
        raised = False
        try:
            async for _ in provider.stream_response("hello"):
                pass
        except ProviderError:
            raised = True

        assert raised, "stream_response must re-raise as ProviderError"


@pytest.mark.unit
class TestOpenAIProviderError:
    @pytest.fixture
    def provider(self):
        from providers.openai import OpenAIProvider

        return OpenAIProvider(api_key="test", model="gpt-4o")

    @pytest.mark.asyncio
    async def test_generate_response_wraps(self, provider):
        provider.client.responses.create = AsyncMock(
            side_effect=_openai_status_error("Incorrect API key provided", 401)
        )

        with pytest.raises(ProviderError) as exc:
            await provider.generate_response("hello")

        assert "Incorrect API key" in exc.value.message
        assert exc.value.provider_type is ProviderType.OPENAI
        assert exc.value.status_code == 401

    @pytest.mark.asyncio
    async def test_stream_response_wraps(self, provider):
        provider.client.responses.create = AsyncMock(
            side_effect=_openai_status_error("rate limited", 429)
        )

        with pytest.raises(ProviderError) as exc:
            async for _ in provider.stream_response("hello"):
                pass

        assert exc.value.status_code == 429
        assert exc.value.provider_type is ProviderType.OPENAI


@pytest.mark.unit
class TestGeminiProviderError:
    @pytest.fixture
    def provider(self):
        from providers.gemini import GeminiProvider

        return GeminiProvider(api_key="test", model="gemini-2.5-flash")

    @pytest.mark.asyncio
    async def test_generate_response_wraps(self, provider):
        provider.client.aio.models.generate_content = AsyncMock(
            side_effect=_gemini_api_error("permission denied", 403)
        )

        with pytest.raises(ProviderError) as exc:
            await provider.generate_response("hello")

        assert "permission denied" in exc.value.message
        assert exc.value.provider_type is ProviderType.GEMINI
        assert exc.value.status_code == 403

    @pytest.mark.asyncio
    async def test_stream_response_wraps(self, provider):
        provider.client.aio.models.generate_content_stream = AsyncMock(
            side_effect=_gemini_api_error("model not found", 404)
        )

        with pytest.raises(ProviderError) as exc:
            async for _ in provider.stream_response("hello"):
                pass

        assert "model not found" in exc.value.message
        assert exc.value.provider_type is ProviderType.GEMINI
        assert exc.value.status_code == 404


@pytest.mark.unit
class TestBedrockProviderError:
    @pytest.fixture
    def provider(self):
        from providers.bedrock import BedrockProvider

        # Anthropic-family path uses AnthropicBedrock which we'll mock per-test.
        with patch("providers.bedrock.AnthropicBedrock") as mock_cls:
            mock_cls.return_value = MagicMock()
            return BedrockProvider(model_id="anthropic.claude-sonnet-4-20250514-v1:0")

    @pytest.mark.asyncio
    async def test_generate_response_wraps_anthropic_sdk_error(self, provider):
        # Reproduces the exact failure mode from the user's report: a 404
        # NotFoundError raised by the AnthropicBedrock client because the
        # account hasn't filled out the model use case form yet.
        provider.client.messages.create = MagicMock(
            side_effect=_anthropic_status_error(
                "Model use case details have not been submitted for this account.",
                404,
            )
        )

        with pytest.raises(ProviderError) as exc:
            await provider.generate_response("hello")

        assert "Model use case details" in exc.value.message
        assert exc.value.provider_type is ProviderType.BEDROCK
        assert exc.value.status_code == 404

    @pytest.mark.asyncio
    async def test_generate_response_wraps_client_error_with_body(self, provider):
        from botocore.exceptions import ClientError

        client_error = ClientError(
            error_response={
                "Error": {
                    "Code": "AccessDeniedException",
                    "Message": "Your IAM user is not authorized.",
                },
                "ResponseMetadata": {"HTTPStatusCode": 403},
            },
            operation_name="InvokeModel",
        )
        provider.client.messages.create = MagicMock(side_effect=client_error)

        with pytest.raises(ProviderError) as exc:
            await provider.generate_response("hello")

        # AWS code AND human-readable body both preserved
        assert "AccessDeniedException" in exc.value.message
        assert "Your IAM user is not authorized" in exc.value.message
        assert exc.value.status_code == 403
        assert exc.value.provider_type is ProviderType.BEDROCK


@pytest.mark.unit
class TestOpenAICompatibleProviderError:
    @pytest.fixture
    def provider(self):
        from providers.openai_compatible import OpenAICompatibleProvider

        return OpenAICompatibleProvider(
            base_url="http://llama-cpp:8000", api_key="x", model="llama-3"
        )

    @pytest.mark.asyncio
    async def test_generate_response_wraps(self, provider):
        provider.client.chat.completions.create = AsyncMock(
            side_effect=_openai_status_error("connection refused", 503)
        )

        with pytest.raises(ProviderError) as exc:
            await provider.generate_response("hello")

        assert "connection refused" in exc.value.message
        assert exc.value.provider_type is ProviderType.OPENAI_COMPATIBLE
        assert exc.value.status_code == 503


def test_provider_error_preserves_explicit_context_overflow_flag():
    err = ProviderError(
        "context_length_exceeded",
        provider_type=ProviderType.OPENAI,
        model="test",
        is_context_overflow=True,
    )
    assert err.is_context_overflow


def test_non_context_provider_error_not_classified_by_default():
    err = ProviderError(
        "authentication failed", provider_type=ProviderType.OPENAI, model="test"
    )
    assert not err.is_context_overflow
