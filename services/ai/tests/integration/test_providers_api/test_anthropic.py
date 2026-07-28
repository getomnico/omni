"""Integration test for the Anthropic provider — one streaming call."""

import os
import pytest

from tests.integration.test_providers_api.conftest import require_env, stream_usage, report_usage
from providers.anthropic import AnthropicProvider

pytestmark = pytest.mark.real_llm

API_KEY = require_env("TEST_ANTHROPIC_API_KEY")
MODEL = os.environ.get("TEST_ANTHROPIC_MODEL", "claude-haiku-4-5")


class TestAnthropic:
    async def test_stream(self) -> None:
        provider = AnthropicProvider(api_key=API_KEY, model=MODEL)
        events = [
            e
            async for e in provider.stream_response(
                "Count from one to three", max_tokens=30
            )
        ]
        assert events[-1].type == "message_stop"
        u = stream_usage(events)
        if u:
            report_usage("Anthropic.stream", u[0], u[1])
