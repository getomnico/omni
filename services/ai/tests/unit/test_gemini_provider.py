from __future__ import annotations

import json

import pytest

from providers.gemini import _convert_messages_to_gemini, _convert_tools_to_gemini

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


def _darwinbox_style_tools() -> list[dict]:
    return [
        {
            "name": "search",
            "description": "Search the Omni index",
            "input_schema": {
                "type": "object",
                "properties": {"query": {"type": "string"}},
                "required": ["query"],
            },
        },
        {
            "name": "darwinbox__attendance",
            "description": "Fetch attendance",
            "input_schema": {
                "type": "object",
                "properties": {"year": {"type": "string"}},
                "additionalProperties": False,
            },
        },
        {
            "name": "darwinbox__apply_leave",
            "description": "Apply for leave",
            "input_schema": {
                "type": "object",
                "properties": {
                    "leave_name": {
                        "type": "string",
                        "enum": ["Casual", "Sick"],
                        "default": "Casual",
                    },
                    "tags": {"type": "array", "items": {"type": "string"}},
                },
                "required": ["leave_name"],
                "additionalProperties": False,
                "additional_properties": False,
                "minItems": 1,
                "min_items": 1,
                "oneOf": [{"type": "string"}],
                "anyOf": [{"type": "string"}],
                "$schema": "http://json-schema.org/draft-07/schema#",
            },
        },
    ]


def test_convert_tools_strips_schema_keys_gemini_cannot_round_trip():
    converted = _convert_tools_to_gemini(_darwinbox_style_tools())
    declarations = converted[0].function_declarations
    assert [d.name for d in declarations] == [
        "search",
        "darwinbox__attendance",
        "darwinbox__apply_leave",
    ]

    # model_dump() with default by_alias=False is exactly how the genai SDK
    # serializes Schema fields to the wire, so any key that survives must be
    # one the API accepts as-is (no snake_case field names, no extras).
    dumped = json.dumps(
        [d.model_dump(mode="json", exclude_none=True) for d in declarations]
    )
    assert "additionalProperties" not in dumped
    assert "additional_properties" not in dumped
    assert "oneOf" not in dumped
    assert "anyOf" not in dumped
    assert "any_of" not in dumped
    assert "minItems" not in dumped
    assert "min_items" not in dumped
    assert "$schema" not in dumped

    params = declarations[2].parameters
    assert params.type == "OBJECT"
    assert params.required == ["leave_name"]
    assert params.properties["leave_name"].enum == ["Casual", "Sick"]
    assert params.properties["leave_name"].default == "Casual"
    assert params.properties["tags"].items.type == "STRING"


def test_gemini_wire_payload_has_no_snake_case_schema_keys():
    """Regression test for the 400 INVALID_ARGUMENT on function_declarations[N].parameters.

    Connector action schemas declare ``additionalProperties: false``; the genai
    SDK validates parameters into its Schema model and serializes it with
    model_dump(by_alias=False), which turns the field into
    ``additional_properties`` on the wire. The API rejects that unknown name.
    """
    from google import genai
    from google.genai import types
    import google.genai._common as _common
    import google.genai.models as _models

    client = genai.Client(api_key="fake")
    config = types.GenerateContentConfig(max_output_tokens=4096)
    config.tools = _convert_tools_to_gemini(_darwinbox_style_tools())
    parameters = types._GenerateContentParameters(
        model="gemini-2.5-flash", contents="hi", config=config
    )

    request_dict = _models._GenerateContentParameters_to_mldev(
        client, parameters, None, parameters
    )
    request_dict.pop("config", None)
    request_dict = _common.convert_to_dict(request_dict)
    request_dict = _common.encode_unserializable_types(request_dict)
    wire = json.dumps(request_dict)

    assert "additional_properties" not in wire
    assert "additionalProperties" not in wire
    assert "oneOf" not in wire
    assert "min_items" not in wire

    tool = request_dict["tools"][0]["functionDeclarations"]
    attendance = next(d for d in tool if d["name"] == "darwinbox__attendance")
    assert attendance["parameters"] == {
        "type": "OBJECT",
        "properties": {"year": {"type": "STRING"}},
    }


def test_convert_messages_handles_only_user_text_documents():
    text_document = {
        "type": "document",
        "title": "Report.pdf",
        "source": {"type": "text", "data": "Q3 revenue grew 14%."},
    }
    binary_document = {
        "type": "document",
        "source": {"type": "base64", "data": "ignored"},
    }

    converted = _convert_messages_to_gemini(
        [
            {"role": "user", "content": [text_document, binary_document]},
            {"role": "assistant", "content": [text_document]},
        ]
    )

    assert len(converted) == 1
    assert converted[0].role == "user"
    assert [part.text for part in converted[0].parts] == [
        'Document title: "Report.pdf"\nDocument content:\nQ3 revenue grew 14%.'
    ]
