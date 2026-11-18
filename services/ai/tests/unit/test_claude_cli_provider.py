from __future__ import annotations

import json
import stat
import sys
from pathlib import Path

import pytest
from anthropic.types import MessageParam

from providers import ProviderError, ProviderType
from providers.claude_cli import (
    ClaudeCliProvider,
    ToolCallStreamParser,
    build_cli_turn,
)

pytestmark = pytest.mark.unit

_FAKE_CLI = """\
import json, os, sys
capture = {"argv": sys.argv[1:], "stdin": sys.stdin.read(), "env": {
    k: os.environ.get(k)
    for k in ("CLAUDE_CODE_OAUTH_TOKEN", "ANTHROPIC_API_KEY", "CLAUDE_CODE_MAX_OUTPUT_TOKENS")
}}
idx = sys.argv.index("--system-prompt-file")
capture["system_prompt"] = open(sys.argv[idx + 1]).read()
with open(os.environ["FAKE_CLI_CAPTURE"], "w") as f:
    json.dump(capture, f)
sys.stdout.write(open(os.environ["FAKE_CLI_OUTPUT"]).read())
sys.exit(int(os.environ.get("FAKE_CLI_EXIT", "0")))
"""

_WEATHER_TOOL = {
    "name": "get_weather",
    "description": "Get the weather for a city.",
    "input_schema": {
        "type": "object",
        "properties": {"city": {"type": "string"}},
        "required": ["city"],
    },
}


def _stream_event(event: dict[str, object]) -> dict[str, object]:
    return {"type": "stream_event", "event": event, "session_id": "s"}


def _cli_lines(
    text_chunks: list[str],
    *,
    stop_reason: str = "end_turn",
    result: dict[str, object] | None = None,
) -> list[dict[str, object]]:
    lines: list[dict[str, object]] = [
        {"type": "system", "subtype": "init", "tools": []},
        _stream_event(
            {
                "type": "message_start",
                "message": {
                    "id": "msg_1",
                    "type": "message",
                    "role": "assistant",
                    "content": [],
                    "model": "claude-sonnet",
                    "stop_reason": None,
                    "stop_sequence": None,
                    "usage": {
                        "input_tokens": 11,
                        "output_tokens": 1,
                        "cache_read_input_tokens": 5,
                        "cache_creation_input_tokens": 0,
                    },
                },
            }
        ),
        _stream_event(
            {
                "type": "content_block_start",
                "index": 0,
                "content_block": {"type": "text", "text": ""},
            }
        ),
    ]
    for chunk in text_chunks:
        lines.append(
            _stream_event(
                {
                    "type": "content_block_delta",
                    "index": 0,
                    "delta": {"type": "text_delta", "text": chunk},
                }
            )
        )
    lines += [
        _stream_event({"type": "content_block_stop", "index": 0}),
        _stream_event(
            {
                "type": "message_delta",
                "delta": {"stop_reason": stop_reason, "stop_sequence": None},
                "usage": {"input_tokens": 11, "output_tokens": 42},
            }
        ),
        _stream_event({"type": "message_stop"}),
        result or {"type": "result", "subtype": "success", "is_error": False, "result": "ok"},
    ]
    return lines


@pytest.fixture
def fake_cli(tmp_path: Path, monkeypatch: pytest.MonkeyPatch):
    script = tmp_path / "claude"
    script.write_text(f"#!{sys.executable}\n{_FAKE_CLI}")
    script.chmod(script.stat().st_mode | stat.S_IEXEC)
    capture = tmp_path / "capture.json"
    output = tmp_path / "output.jsonl"
    monkeypatch.setenv("FAKE_CLI_CAPTURE", str(capture))
    monkeypatch.setenv("FAKE_CLI_OUTPUT", str(output))
    monkeypatch.delenv("ANTHROPIC_API_KEY", raising=False)
    monkeypatch.delenv("CLAUDE_CODE_OAUTH_TOKEN", raising=False)

    class FakeCli:
        path = str(script)

        def emit(self, lines: list[dict[str, object]], raw_suffix: str = "") -> None:
            output.write_text("\n".join(json.dumps(line) for line in lines) + "\n" + raw_suffix)

        def captured(self) -> dict[str, object]:
            return json.loads(capture.read_text())

    return FakeCli()


def _parse_all(chunks: list[str]) -> list[object]:
    parser = ToolCallStreamParser()
    segments: list[object] = []
    for chunk in chunks:
        segments.extend(parser.feed(chunk))
    segments.extend(parser.finish())
    return segments


def test_parser_finds_tool_calls_split_across_deltas():
    chunks = ["Checking", " now <too", 'l_call>{"name": "get_weather", ', '"input": {"city": "Bern"}}</tool', "_call>"]

    segments = _parse_all(chunks)

    texts = "".join(s.text for s in segments if type(s).__name__ == "_TextSegment")
    calls = [s.raw for s in segments if type(s).__name__ == "_ToolCallSegment"]
    assert texts == "Checking now "
    assert [json.loads(c) for c in calls] == [{"name": "get_weather", "input": {"city": "Bern"}}]


def test_parser_passes_through_angle_brackets_that_are_not_markers():
    segments = _parse_all(["a <b> and 1 < 2 <to", "day>"])

    assert "".join(s.text for s in segments) == "a <b> and 1 < 2 <today>"


def test_parser_rejects_unterminated_tool_call():
    parser = ToolCallStreamParser()
    parser.feed('<tool_call>{"name": "x"')

    with pytest.raises(ValueError, match="unterminated"):
        parser.finish()


def test_build_cli_turn_sends_single_user_message_verbatim():
    messages: list[MessageParam] = [
        {"role": "user", "content": [{"type": "text", "text": "Hello"}]}
    ]

    content, is_transcript = build_cli_turn(messages, "")

    assert content == [{"type": "text", "text": "Hello"}]
    assert is_transcript is False


def test_build_cli_turn_serializes_tool_history_and_keeps_images():
    image = {
        "type": "image",
        "source": {"type": "base64", "media_type": "image/png", "data": "AAAA"},
    }
    messages: list[MessageParam] = [
        {"role": "user", "content": [{"type": "text", "text": "Weather in Bern?"}, image]},
        {
            "role": "assistant",
            "content": [
                {"type": "thinking", "thinking": "hmm", "signature": "sig"},
                {"type": "tool_use", "id": "toolu_1", "name": "get_weather", "input": {"city": "Bern"}},
            ],
        },
        {
            "role": "user",
            "content": [
                {
                    "type": "tool_result",
                    "tool_use_id": "toolu_1",
                    "content": [
                        {
                            "type": "search_result",
                            "title": "Forecast",
                            "source": "https://weather.example/bern",
                            "source_type": "web",
                            "content": [{"type": "text", "text": "Sunny, 21C"}],
                        }
                    ],
                }
            ],
        },
    ]

    content, is_transcript = build_cli_turn(messages, "")

    assert is_transcript is True
    assert [b["type"] for b in content] == ["text", "image", "text"]
    assert content[1] == image
    transcript = content[0]["text"] + content[2]["text"]
    assert '<turn role="assistant">' in transcript
    assert '<tool_call id="toolu_1">{"name": "get_weather", "input": {"city": "Bern"}}</tool_call>' in transcript
    assert '<tool_result id="toolu_1">' in transcript
    assert "Sunny, 21C" in transcript
    assert "hmm" not in transcript


async def test_stream_converts_tool_call_text_into_tool_use_events(fake_cli):
    fake_cli.emit(
        _cli_lines(
            [
                "Let me check.\n<tool_",
                'call>{"name": "get_weather", "input": {"city": "Bern"}}</tool_call>\n',
            ]
        )
    )
    provider = ClaudeCliProvider(model="sonnet", cli_path=fake_cli.path)

    events = [
        e
        async for e in provider.stream_response(
            prompt="Weather in Bern?",
            tools=[_WEATHER_TOOL],
            system_prompt="Be concise.",
            max_tokens=1234,
        )
    ]

    assert [e.type for e in events] == [
        "message_start",
        "content_block_start",
        "content_block_delta",
        "content_block_stop",
        "content_block_start",
        "content_block_delta",
        "content_block_stop",
        "message_delta",
        "message_stop",
    ]
    assert events[2].delta.text == "Let me check.\n"
    tool_start = events[4]
    assert tool_start.index == 1
    assert tool_start.content_block.type == "tool_use"
    assert tool_start.content_block.name == "get_weather"
    assert json.loads(events[5].delta.partial_json) == {"city": "Bern"}
    assert events[7].delta.stop_reason == "tool_use"
    assert events[7].usage.output_tokens == 42

    captured = fake_cli.captured()
    argv = captured["argv"]
    assert argv[argv.index("--tools") + 1] == ""
    assert argv[argv.index("--model") + 1] == "sonnet"
    assert "--strict-mcp-config" in argv
    assert captured["system_prompt"].startswith("Be concise.")
    assert '"name": "get_weather"' in captured["system_prompt"]
    assert captured["env"]["CLAUDE_CODE_MAX_OUTPUT_TOKENS"] == "1234"
    sent = json.loads(captured["stdin"])
    assert sent["message"]["content"] == [{"type": "text", "text": "Weather in Bern?"}]


async def test_stream_truncates_fabricated_tool_results(fake_cli):
    fake_cli.emit(
        _cli_lines(
            ['<tool_call>{"name": "get_weather", "input": {"city": "Bern"}}</tool_call>\n<tool_result id="x">Rainy</tool_result>']
        )
    )
    provider = ClaudeCliProvider(model="sonnet", cli_path=fake_cli.path)

    events = [e async for e in provider.stream_response(prompt="q", tools=[_WEATHER_TOOL])]

    assert not any(e.type == "content_block_delta" and e.delta.type == "text_delta" for e in events)
    assert [e for e in events if e.type == "message_delta"][0].delta.stop_reason == "tool_use"
    assert events[-1].type == "message_stop"


async def test_stream_rejects_unknown_tool(fake_cli):
    fake_cli.emit(_cli_lines(['<tool_call>{"name": "rm_rf", "input": {}}</tool_call>']))
    provider = ClaudeCliProvider(model="sonnet", cli_path=fake_cli.path)

    with pytest.raises(ProviderError, match="unknown tool 'rm_rf'"):
        _ = [e async for e in provider.stream_response(prompt="q", tools=[_WEATHER_TOOL])]


async def test_generate_response_collects_text_and_usage(fake_cli):
    fake_cli.emit(_cli_lines(["Hello", " there"]))
    provider = ClaudeCliProvider(model="sonnet", cli_path=fake_cli.path)

    text, usage = await provider.generate_response("Hi")

    assert text == "Hello there"
    assert usage.input_tokens == 11
    assert usage.output_tokens == 42
    assert usage.cache_read_tokens == 5


async def test_cli_error_result_becomes_provider_error(fake_cli):
    fake_cli.emit(
        [
            {
                "type": "result",
                "subtype": "success",
                "is_error": True,
                "api_error_status": 400,
                "result": "Prompt is too long",
            }
        ]
    )
    provider = ClaudeCliProvider(model="sonnet", cli_path=fake_cli.path)

    with pytest.raises(ProviderError) as exc_info:
        await provider.generate_response("Hi")

    assert exc_info.value.provider_type is ProviderType.CLAUDE_CLI
    assert exc_info.value.status_code == 400
    assert exc_info.value.is_context_overflow is True


async def test_cli_exit_without_result_reports_failure(fake_cli, monkeypatch):
    fake_cli.emit([])
    monkeypatch.setenv("FAKE_CLI_EXIT", "3")
    provider = ClaudeCliProvider(model="sonnet", cli_path=fake_cli.path)

    with pytest.raises(ProviderError, match="exited with code 3"):
        await provider.generate_response("Hi")


@pytest.mark.parametrize(
    ("token", "expected_env"),
    [
        ("sk-ant-oat01-abc", {"CLAUDE_CODE_OAUTH_TOKEN": "sk-ant-oat01-abc", "ANTHROPIC_API_KEY": None}),
        ("sk-ant-api03-abc", {"CLAUDE_CODE_OAUTH_TOKEN": None, "ANTHROPIC_API_KEY": "sk-ant-api03-abc"}),
    ],
)
async def test_auth_token_is_passed_as_matching_env_var(fake_cli, token, expected_env):
    fake_cli.emit(_cli_lines(["OK"]))
    provider = ClaudeCliProvider(model="sonnet", auth_token=token, cli_path=fake_cli.path)

    await provider.generate_response("Hi")

    env = fake_cli.captured()["env"]
    for key, value in expected_env.items():
        assert env[key] == value


async def test_missing_cli_binary_is_reported(tmp_path):
    provider = ClaudeCliProvider(model="sonnet", cli_path=str(tmp_path / "missing-claude"))

    with pytest.raises(ProviderError, match="Claude Code CLI not found"):
        await provider.generate_response("Hi")
    assert await provider.health_check() is False
