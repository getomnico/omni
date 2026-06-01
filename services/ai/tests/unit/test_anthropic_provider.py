from __future__ import annotations

import pytest

from providers.anthropic import AnthropicProvider

pytestmark = pytest.mark.unit


def test_build_messages_for_api_strips_extra_search_result_fields_without_mutating_source():
    provider = AnthropicProvider(api_key="test-key", model="test-model")
    messages = [
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
                            "source": "https://jira.example/browse/PROJ-1",
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

    api_messages = provider.build_messages_for_api(messages)

    internal_search_result = messages[0]["content"][0]["content"][0]
    api_search_result = api_messages[0]["content"][0]["content"][0]
    assert internal_search_result["source_type"] == "jira"
    assert internal_search_result["internal_extra"] == "must-not-be-sent"
    assert "source_type" not in api_search_result
    assert "internal_extra" not in api_search_result
    assert api_search_result == {
        "type": "search_result",
        "title": "Issue",
        "source": "https://jira.example/browse/PROJ-1",
        "content": [{"type": "text", "text": "body"}],
        "citations": {"enabled": True},
    }
