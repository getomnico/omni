from __future__ import annotations

import json

import pytest

from providers.openai import OpenAIProvider

pytestmark = pytest.mark.unit


def test_convert_messages_does_not_forward_search_result_extras():
    provider = OpenAIProvider.__new__(OpenAIProvider)
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

    converted = provider._convert_messages(messages)

    encoded = json.dumps(converted)
    assert "source_type" not in encoded
    assert "must-not-be-sent" not in encoded
    assert converted == [
        {
            "type": "function_call_output",
            "call_id": "call-1",
            "output": "[Issue](https://example.invalid/issue)\nbody",
        }
    ]

    internal_search_result = messages[0]["content"][0]["content"][0]
    assert internal_search_result["source_type"] == "jira"
    assert internal_search_result["internal_extra"] == "must-not-be-sent"
