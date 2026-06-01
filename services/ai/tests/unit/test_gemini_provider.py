from __future__ import annotations

import json

import pytest

from providers.gemini import _convert_messages_to_gemini

pytestmark = pytest.mark.unit


def test_convert_messages_does_not_forward_search_result_extras():
    messages = [
        {
            "role": "user",
            "content": [
                {
                    "type": "tool_result",
                    "tool_use_id": "call-1",
                    "content": [
                        {
                            "type": "search_result",
                            "title": "Issue",
                            "source": "https://example.invalid/issue",
                            "source_type": "jira",
                            "internal_extra": "must-not-be-sent",
                            "content": [{"type": "text", "text": "body"}],
                        }
                    ],
                }
            ],
        }
    ]

    converted = _convert_messages_to_gemini(messages)

    encoded = json.dumps(
        [content.model_dump(mode="json", exclude_none=True) for content in converted]
    )
    assert "source_type" not in encoded
    assert "must-not-be-sent" not in encoded
    assert "[Issue](https://example.invalid/issue)\\nbody" in encoded

    internal_search_result = messages[0]["content"][0]["content"][0]
    assert internal_search_result["source_type"] == "jira"
    assert internal_search_result["internal_extra"] == "must-not-be-sent"
