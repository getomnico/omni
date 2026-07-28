"""Integration test for the OpenAI provider — one streaming call."""

import os
import pytest

from tests.integration.test_providers_api.conftest import require_env, stream_usage, report_usage
from providers.openai import OpenAIProvider

pytestmark = pytest.mark.real_llm

API_KEY = require_env("TEST_OPENAI_API_KEY")
MODEL = os.environ.get("TEST_OPENAI_MODEL", "gpt-4o")


class TestOpenAI:
    async def test_stream(self) -> None:
        provider = OpenAIProvider(api_key=API_KEY, model=MODEL)
        events = [
            e
            async for e in provider.stream_response(
                "Count from one to three", max_tokens=30
            )
        ]
        assert events[-1].type == "message_stop"
        u = stream_usage(events)
        if u:
            report_usage("OpenAI.stream", u[0], u[1])
