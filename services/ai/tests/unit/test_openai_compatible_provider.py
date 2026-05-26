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


def test_get_passthrough_delta_value_reads_pydantic_extra():
    class Delta:
        model_extra = {REASONING_CONTENT_KEY: "reasoning delta"}

    assert _get_passthrough_delta_value(Delta(), REASONING_CONTENT_KEY) == "reasoning delta"
