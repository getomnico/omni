"""Unit tests for tool-input parsing in the streaming pipeline."""

from anthropic.types import ToolUseBlockParam

from streaming.persist import parse_tool_call_inputs


def test_empty_tool_input_is_valid_empty_object():
    """Anthropic streams an empty ``input_json_delta`` for no-arg tool calls;
    the resulting empty input must become ``{}`` without a parse error."""
    tool_call = ToolUseBlockParam(
        type="tool_use", id="toolu_empty", name="no_arg_tool", input=""
    )
    errors = parse_tool_call_inputs([tool_call])

    assert errors == []
    assert tool_call["input"] == {}


def test_whitespace_tool_input_is_valid_empty_object():
    tool_call = ToolUseBlockParam(
        type="tool_use", id="toolu_ws", name="no_arg_tool", input="   "
    )
    errors = parse_tool_call_inputs([tool_call])

    assert errors == []
    assert tool_call["input"] == {}


def test_valid_json_input_is_parsed():
    tool_call = ToolUseBlockParam(
        type="tool_use",
        id="toolu_json",
        name="search",
        input='{"query": "leave balance"}',
    )
    errors = parse_tool_call_inputs([tool_call])

    assert errors == []
    assert tool_call["input"] == {"query": "leave balance"}


def test_invalid_json_input_yields_parse_error():
    tool_call = ToolUseBlockParam(
        type="tool_use", id="toolu_bad", name="search", input="{nope"
    )
    errors = parse_tool_call_inputs([tool_call])

    assert len(errors) == 1
    assert errors[0]["tool_use_id"] == "toolu_bad"
    assert errors[0]["is_error"] is True
