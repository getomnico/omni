from __future__ import annotations

import json

import pytest

from providers.bedrock import BedrockProvider

pytestmark = pytest.mark.unit


def _message_with_search_result_extra_fields():
    return [
        {
            "role": "user",
            "content": [
                {
                    "type": "tool_result",
                    "tool_use_id": "tool-1",
                    "content": [
                        {
                            "type": "search_result",
                            "title": "Issue",
                            "source": "https://example.invalid/issue",
                            "source_type": "jira",
                            "internal_extra": "must-not-be-sent",
                            "content": [{"type": "text", "text": "body"}],
                            "citations": {"enabled": True},
                        }
                    ],
                    "is_error": False,
                }
            ],
        }
    ]


class _RecordingMessagesClient:
    def __init__(self) -> None:
        self.request_params = None

    def create(self, **kwargs):
        self.request_params = kwargs
        return []


class _RecordingAnthropicBedrockClient:
    def __init__(self) -> None:
        self.messages = _RecordingMessagesClient()


@pytest.mark.asyncio
async def test_anthropic_stream_sanitizes_search_result_extras_without_mutating_source():
    provider = BedrockProvider.__new__(BedrockProvider)
    provider.model_id = "anthropic.claude-3-5-sonnet-20241022-v2:0"
    provider.model_family = "anthropic"
    provider.client = _RecordingAnthropicBedrockClient()

    messages = _message_with_search_result_extra_fields()

    events = [
        event
        async for event in provider.stream_response(prompt="ignored", messages=messages)
    ]

    assert events == []
    api_search_result = provider.client.messages.request_params["messages"][0]["content"][0][
        "content"
    ][0]
    assert api_search_result == {
        "type": "search_result",
        "title": "Issue",
        "source": "https://example.invalid/issue",
        "content": [{"type": "text", "text": "body"}],
        "citations": {"enabled": True},
    }

    internal_search_result = messages[0]["content"][0]["content"][0]
    assert internal_search_result["source_type"] == "jira"
    assert internal_search_result["internal_extra"] == "must-not-be-sent"


def test_amazon_message_adapter_does_not_forward_search_result_extras():
    provider = BedrockProvider.__new__(BedrockProvider)
    messages = _message_with_search_result_extra_fields()

    adapted = provider._adapt_messages_for_amazon_models(messages)

    encoded = json.dumps(adapted)
    assert "source_type" not in encoded
    assert "must-not-be-sent" not in encoded
    assert "toolResult" in encoded
    assert "document" in encoded

    internal_search_result = messages[0]["content"][0]["content"][0]
    assert internal_search_result["source_type"] == "jira"
    assert internal_search_result["internal_extra"] == "must-not-be-sent"


def test_amazon_message_adapter_preserves_top_level_document_block():
    provider = BedrockProvider.__new__(BedrockProvider)
    messages = [
        {
            "role": "user",
            "content": [
                {
                    "type": "document",
                    "source": {"type": "text", "media_type": "text/plain", "data": "Document body"},
                    "title": "Report.pdf",
                },
                {"type": "text", "text": "Summarize this"},
            ],
        }
    ]

    adapted = provider._adapt_messages_for_amazon_models(messages)

    adapted_blocks = adapted[0]["content"]
    assert any("Document body" in block.get("text", "") for block in adapted_blocks)


def test_amazon_message_adapter_drops_non_text_document_block():
    provider = BedrockProvider.__new__(BedrockProvider)
    messages = [
        {
            "role": "user",
            "content": [
                {
                    "type": "document",
                    "source": {"type": "base64", "media_type": "application/pdf", "data": "AAAA"},
                    "title": "binary.pdf",
                },
            ],
        }
    ]

    adapted = provider._adapt_messages_for_amazon_models(messages)

    adapted_blocks = adapted[0]["content"]
    assert all(block.get("type") != "document" for block in adapted_blocks)


def test_amazon_message_adapter_handles_document_with_title():
    provider = BedrockProvider.__new__(BedrockProvider)
    messages = [
        {
            "role": "user",
            "content": [
                {
                    "type": "document",
                    "source": {"type": "text", "media_type": "text/plain", "data": "The content"},
                    "title": "Meeting Notes",
                },
            ],
        }
    ]

    adapted = provider._adapt_messages_for_amazon_models(messages)

    adapted_blocks = adapted[0]["content"]
    assert any("Meeting Notes" in block.get("text", "") for block in adapted_blocks)
    assert any("The content" in block.get("text", "") for block in adapted_blocks)


def test_amazon_message_adapter_omits_document_in_assistant_role():
    provider = BedrockProvider.__new__(BedrockProvider)
    messages = [
        {
            "role": "assistant",
            "content": [
                {
                    "type": "document",
                    "source": {"type": "text", "media_type": "text/plain", "data": "Assistant body"},
                    "title": "Doc.pdf",
                },
            ],
        }
    ]

    adapted = provider._adapt_messages_for_amazon_models(messages)

    adapted_blocks = adapted[0]["content"]
    assert all("Assistant body" not in block.get("text", "") for block in adapted_blocks)
