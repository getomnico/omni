"""Integration test for the OpenAI-compatible provider — one streaming call."""

import os
import pytest

from tests.integration.test_providers_api.conftest import require_env, stream_usage, report_usage
from providers.openai_compatible import OpenAICompatibleProvider

pytestmark = pytest.mark.real_llm

BASE_URL = require_env("TEST_OPENAI_COMPATIBLE_BASE_URL")
API_KEY = require_env("TEST_OPENAI_COMPATIBLE_API_KEY")
MODEL = os.environ.get("TEST_OPENAI_COMPATIBLE_MODEL", "gpt-4o-mini")


class TestOpenAICompatible:
    async def test_stream(self) -> None:
        provider = OpenAICompatibleProvider(
            base_url=BASE_URL, api_key=API_KEY, model=MODEL
        )
        events = [
            e
            async for e in provider.stream_response(
                "Count from one to three", max_tokens=30
            )
        ]
        assert events[-1].type == "message_stop"
        u = stream_usage(events)
        if u:
            report_usage("OpenAICompat.stream", u[0], u[1])
