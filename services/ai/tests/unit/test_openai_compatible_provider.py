from __future__ import annotations

import json

import pytest

from providers.openai_compatible import (
    REASONING_CONTENT_KEY,
    _convert_messages_to_openai,
    _get_passthrough_delta_value,
    validate_openai_tool_message_sequence,
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

    assert (
        _get_passthrough_delta_value(Delta(), REASONING_CONTENT_KEY)
        == "reasoning delta"
    )


def _assistant_with_tool_calls(tool_call_ids: list[str]) -> dict:
    return {
        "role": "assistant",
        "content": [
            {
                "type": "tool_use",
                "id": tool_id,
                "name": "search_documents",
                "input": {},
            }
            for tool_id in tool_call_ids
        ],
    }


def _tool_message(tool_call_id: str) -> dict:
    return {
        "role": "tool",
        "tool_call_id": tool_call_id,
        "content": "result",
    }


def _openai_assistant_with_tool_calls(tool_call_ids: list[str]) -> dict:
    """An OpenAI-format assistant message carrying tool_calls (as the validator
    sees them post-conversion)."""
    return {
        "role": "assistant",
        "content": None,
        "tool_calls": [
            {
                "id": tool_id,
                "type": "function",
                "function": {"name": "search_documents", "arguments": "{}"},
            }
            for tool_id in tool_call_ids
        ],
    }


def test_validate_accepts_parallel_tool_messages():
    converted = _convert_messages_to_openai(
        [
            {"role": "user", "content": "hi"},
            _assistant_with_tool_calls(["call_A", "call_B"]),
            {
                "role": "user",
                "content": [
                    {"type": "tool_result", "tool_use_id": "call_A", "content": "r1"},
                    {"type": "tool_result", "tool_use_id": "call_B", "content": "r2"},
                ],
            },
            _assistant_with_tool_calls(["call_C"]),
            {
                "role": "user",
                "content": [
                    {"type": "tool_result", "tool_use_id": "call_C", "content": "r3"}
                ],
            },
        ]
    )

    validate_openai_tool_message_sequence(converted)


def test_validate_rejects_duplicate_tool_call_id():
    converted = [
        _openai_assistant_with_tool_calls(["call_A"]),
        _tool_message("call_A"),
        _tool_message("call_A"),
    ]

    with pytest.raises(ValueError, match="no matching tool_calls"):
        validate_openai_tool_message_sequence(converted)


def test_validate_rejects_missing_tool_result_before_user_message():
    """An assistant batch must be fully satisfied before any non-tool message."""
    converted = [
        _openai_assistant_with_tool_calls(["call_A", "call_B"]),
        _tool_message("call_A"),
        {"role": "user", "content": "new question"},
    ]

    with pytest.raises(ValueError, match="never answered"):
        validate_openai_tool_message_sequence(converted)


def test_validate_rejects_tool_call_answered_after_user_message():
    """A tool message cannot be paired with an assistant batch across a user
    message boundary."""
    converted = [
        _openai_assistant_with_tool_calls(["call_A"]),
        {"role": "user", "content": "new question"},
        _tool_message("call_A"),
    ]

    with pytest.raises(ValueError, match="never answered"):
        validate_openai_tool_message_sequence(converted)


def test_validate_rejects_tool_message_after_user_message():
    converted = [
        _openai_assistant_with_tool_calls(["call_A"]),
        _tool_message("call_A"),
        {"role": "user", "content": "new question"},
        _tool_message("call_A"),
    ]

    with pytest.raises(ValueError, match="no matching tool_calls"):
        validate_openai_tool_message_sequence(converted)


def test_validate_rejects_consecutive_assistant_tool_call_messages():
    converted = [
        _openai_assistant_with_tool_calls(["call_A"]),
        _openai_assistant_with_tool_calls(["call_B"]),
    ]

    with pytest.raises(ValueError, match="never answered"):
        validate_openai_tool_message_sequence(converted)


def test_validate_rejects_trailing_unanswered_tool_calls():
    converted = [_openai_assistant_with_tool_calls(["call_A"])]

    with pytest.raises(ValueError, match="never answered"):
        validate_openai_tool_message_sequence(converted)


def test_validate_rejects_tool_message_without_preceding_assistant():
    converted = [_tool_message("call_A")]

    with pytest.raises(ValueError, match="no matching tool_calls"):
        validate_openai_tool_message_sequence(converted)


def test_convert_messages_includes_user_text_document():
    document = {
        "type": "document",
        "title": "Report.pdf",
        "source": {"type": "text", "data": "Q3 revenue grew 14%."},
    }

    converted = _convert_messages_to_openai(
        [{"role": "user", "content": [{"type": "text", "text": "Review:"}, document]}]
    )

    assert converted == [
        {
            "role": "user",
            "content": (
                'Review:\nDocument title: "Report.pdf"\n'
                "Document content:\nQ3 revenue grew 14%."
            ),
        }
    ]


def test_convert_messages_omits_assistant_and_unsupported_documents():
    text_document = {
        "type": "document",
        "source": {"type": "text", "data": "untrusted"},
    }
    binary_document = {
        "type": "document",
        "source": {"type": "base64", "data": "ignored"},
    }

    converted = _convert_messages_to_openai(
        [
            {"role": "assistant", "content": [text_document]},
            {
                "role": "user",
                "content": [{"type": "text", "text": "safe"}, binary_document],
            },
        ]
    )

    assert converted == [{"role": "assistant"}, {"role": "user", "content": "safe"}]
