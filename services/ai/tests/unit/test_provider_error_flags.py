"""Unit tests for per-provider transport-error classification.

Each provider sets ``ProviderError.is_retryable`` from its own SDK's
transport exception types. These tests pin that classification: transport
failures must be flagged, deterministic API/local errors must not.
"""

import httpx
import pytest
from anthropic import (
    APIConnectionError as AnthropicConnectionError,
)
from anthropic import (
    APIStatusError as AnthropicStatusError,
)
from botocore.exceptions import (
    ClientError,
    ConnectionClosedError,
    NoCredentialsError,
    ReadTimeoutError,
)
from google.genai.errors import APIError as GenaiAPIError
from openai import APIConnectionError as OpenAIConnectionError
from openai import APIStatusError as OpenAIStatusError

from providers.anthropic import _anthropic_is_retryable
from providers.bedrock import _bedrock_is_retryable
from providers.gemini import _gemini_is_retryable
from providers.openai import _openai_is_retryable
from providers.openai_compatible import _openai_compat_is_retryable

_REQUEST = httpx.Request("POST", "https://provider.test/v1/chat")
_RESPONSE_429 = httpx.Response(429, request=_REQUEST)


def _throttling_client_error() -> ClientError:
    return ClientError(
        {
            "Error": {"Code": "ThrottlingException", "Message": "Slow down"},
            "ResponseMetadata": {"HTTPStatusCode": 429},
        },
        "InvokeModel",
    )


@pytest.mark.unit
class TestTransportErrorClassification:
    def test_httpx_transport_errors_are_retryable_everywhere(self):
        read_error = httpx.ReadError("stream reset", request=_REQUEST)
        connect_error = httpx.ConnectError("connection refused", request=_REQUEST)
        timeout = httpx.ReadTimeout("timed out", request=_REQUEST)
        for classifier in (
            _anthropic_is_retryable,
            _openai_is_retryable,
            _openai_compat_is_retryable,
            _gemini_is_retryable,
        ):
            assert classifier(read_error)
            assert classifier(connect_error)
            assert classifier(timeout)

    def test_anthropic_sdk_connection_error_is_retryable(self):
        assert _anthropic_is_retryable(AnthropicConnectionError(request=_REQUEST))

    def test_openai_sdk_connection_error_is_retryable(self):
        assert _openai_is_retryable(OpenAIConnectionError(request=_REQUEST))

    def test_bedrock_transport_errors_are_retryable(self):
        assert _bedrock_is_retryable(
            ConnectionClosedError(endpoint_url="https://bedrock.test")
        )
        assert _bedrock_is_retryable(
            ReadTimeoutError(endpoint_url="https://bedrock.test")
        )

    def test_anthropic_deterministic_errors_are_not_retryable(self):
        assert not _anthropic_is_retryable(
            AnthropicStatusError("bad key", response=_RESPONSE_429, body=None)
        )
        assert not _anthropic_is_retryable(ValueError("malformed tool message"))

    def test_openai_deterministic_errors_are_not_retryable(self):
        status_error = OpenAIStatusError(
            "rate limited", response=_RESPONSE_429, body=None
        )
        assert not _openai_is_retryable(status_error)
        assert not _openai_compat_is_retryable(status_error)
        assert not _openai_is_retryable(ValueError("malformed tool message"))

    def test_gemini_status_error_is_not_retryable(self):
        assert not _gemini_is_retryable(GenaiAPIError(429, None))
        assert not _gemini_is_retryable(ValueError("malformed request"))

    def test_bedrock_deterministic_errors_are_not_retryable(self):
        assert not _bedrock_is_retryable(_throttling_client_error())
        assert not _bedrock_is_retryable(NoCredentialsError())
        assert not _bedrock_is_retryable(ValueError("unknown model family"))
