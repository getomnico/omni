from __future__ import annotations

import json

from providers.openai_compatible import (
    REASONING_CONTENT_KEY,
    _convert_messages_to_openai,
    _get_passthrough_delta_value,
)


def test_convert_messages_preserves_deepseek_reasoning_content_on_assistant_message():
    messages = [
        {
            "role": "assistant",
            "content": [
                {
                    "type": "text",
                    "text": "Final answer",
                },
                {
                    "type": "text",
                    "text": "",
                    REASONING_CONTENT_KEY: "private chain of thought token",
                },
            ],
        }
    ]

    converted = _convert_messages_to_openai(messages)

    assert converted == [
        {
            "role": "assistant",
            "content": "Final answer",
            REASONING_CONTENT_KEY: "private chain of thought token",
        }
    ]


def test_convert_messages_preserves_deepseek_reasoning_content_with_tool_calls():
    messages = [
        {
            "role": "assistant",
            "content": [
                {
                    "type": "tool_use",
                    "id": "call_1",
                    "name": "search_documents",
                    "input": {"query": "omni"},
                },
                {
                    "type": "text",
                    "text": "",
                    REASONING_CONTENT_KEY: "tool reasoning token",
                },
            ],
        }
    ]

    converted = _convert_messages_to_openai(messages)

    assert len(converted) == 1
    assistant_message = converted[0]
    assert assistant_message["role"] == "assistant"
    assert assistant_message[REASONING_CONTENT_KEY] == "tool reasoning token"
    assert "content" not in assistant_message
    assert assistant_message["tool_calls"][0]["id"] == "call_1"


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

    converted = _convert_messages_to_openai(messages)

    encoded = json.dumps(converted)
    assert "source_type" not in encoded
    assert "must-not-be-sent" not in encoded
    assert converted == [
        {
            "role": "tool",
            "tool_call_id": "call-1",
            "content": "[Issue](https://example.invalid/issue)\nbody",
        }
    ]

    internal_search_result = messages[0]["content"][0]["content"][0]
    assert internal_search_result["source_type"] == "jira"
    assert internal_search_result["internal_extra"] == "must-not-be-sent"


def test_get_passthrough_delta_value_reads_pydantic_extra():
    class Delta:
        model_extra = {REASONING_CONTENT_KEY: "reasoning delta"}

    assert _get_passthrough_delta_value(Delta(), REASONING_CONTENT_KEY) == "reasoning delta"
