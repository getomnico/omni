"""Claude Code CLI provider.

Runs each model call as a one-shot ``claude -p`` subprocess. Claude Code is an
agent with its own tool loop, but Omni must stay in control of tool execution,
so every built-in tool and MCP server is disabled and the CLI is used as a
plain completion engine:

* Omni's tool schemas go into the system prompt; the model requests a tool by
  writing ``<tool_call>{json}</tool_call>``, which this provider converts into
  regular Anthropic ``tool_use`` stream events.
* The CLI only accepts a single new user turn, so the conversation history
  (including earlier tool calls and results) is serialized into that turn.
"""

import asyncio
import json
import logging
import os
import tempfile
import uuid
from collections.abc import AsyncIterator, Iterable
from contextlib import aclosing
from dataclasses import dataclass
from pathlib import Path
from typing import Any, ClassVar, Literal, cast

from anthropic import MessageStreamEvent
from anthropic.types import (
    ContentBlockParam,
    InputJSONDelta,
    MessageDeltaUsage,
    MessageParam,
    RawContentBlockDeltaEvent,
    RawContentBlockStartEvent,
    RawContentBlockStopEvent,
    RawMessageDeltaEvent,
    RawMessageStartEvent,
    RawMessageStopEvent,
    RawMessageStreamEvent,
    TextBlock,
    TextDelta,
    ToolUseBlock,
)
from anthropic.types.raw_message_delta_event import Delta
from pydantic import BaseModel, ConfigDict, TypeAdapter, ValidationError

from . import LLMProvider, TokenUsage
from .anthropic_message_adapter import (
    build_messages_for_anthropic_api,
    extract_text_document,
)
from .types import ProviderError, ProviderType

logger = logging.getLogger(__name__)

DEFAULT_CLI_MODEL = "sonnet"
OAUTH_TOKEN_PREFIX = "sk-ant-oat"
# A single stdout line carries a whole assistant message, which can exceed
# asyncio's 64 KiB default line limit.
_STDOUT_LINE_LIMIT = 64 * 1024 * 1024
_STDERR_TAIL_CHARS = 2000

_TOOL_CALL_OPEN = "<tool_call>"
_TOOL_CALL_CLOSE = "</tool_call>"
# The model sometimes keeps going after a tool call and invents the result.
# Seeing this marker means everything after it is fabricated.
_TOOL_RESULT_MARKER = "<tool_result"

_RAW_EVENT_ADAPTER: TypeAdapter[RawMessageStreamEvent] = TypeAdapter(
    RawMessageStreamEvent
)

_TOOLS_INSTRUCTIONS = """

# Tools

You can call the tools listed below. Each entry gives the tool's name, description and a JSON schema for its input.

<tools>
{tool_specs}
</tools>

To call a tool, write a block in exactly this form, with one JSON object inside:
<tool_call>{{"name": "<tool name>", "input": {{<arguments matching the schema>}}}}</tool_call>

Rules:
- You may call several tools in one reply by writing several <tool_call> blocks.
- After your last <tool_call> block, stop writing. The results arrive in the next user turn as <tool_result id="..."> blocks. Never write a <tool_result> block yourself.
- Only the tools listed above exist. You have no other tools, no shell and no file access.
- If you do not need a tool, answer directly without any <tool_call> block."""

_TRANSCRIPT_INSTRUCTIONS = """

# Conversation format

The user message contains the conversation so far. Each turn is wrapped in <turn role="user"> or <turn role="assistant">. Your earlier tool calls appear as <tool_call id="..."> blocks and their results as <tool_result id="..."> blocks. Write only the next assistant turn, without a <turn> wrapper."""


class _CliResultLine(BaseModel):
    model_config = ConfigDict(extra="ignore")

    type: Literal["result"]
    is_error: bool
    result: str | None = None
    api_error_status: int | None = None


class _ToolCallPayload(BaseModel):
    model_config = ConfigDict(extra="forbid")

    name: str
    input: dict[str, Any]


@dataclass(frozen=True)
class _TextSegment:
    text: str


@dataclass(frozen=True)
class _ToolCallSegment:
    raw: str


@dataclass(frozen=True)
class _FabricatedResultSegment:
    pass


type _Segment = _TextSegment | _ToolCallSegment | _FabricatedResultSegment


def _held_back_suffix_len(buffer: str, markers: Iterable[str]) -> int:
    """Length of the longest buffer suffix that could still grow into a marker."""
    longest = 0
    for marker in markers:
        for size in range(min(len(buffer), len(marker) - 1), 0, -1):
            if marker.startswith(buffer[-size:]):
                longest = max(longest, size)
                break
    return longest


class ToolCallStreamParser:
    """Splits streamed model text into plain text and ``<tool_call>`` blocks.

    Text that could be the start of a marker is held back until enough of the
    stream has arrived to decide, so markers split across deltas are found.
    """

    def __init__(self) -> None:
        self._buffer = ""
        self._in_call = False

    def feed(self, text: str) -> list[_Segment]:
        self._buffer += text
        segments: list[_Segment] = []
        while True:
            if self._in_call:
                end = self._buffer.find(_TOOL_CALL_CLOSE)
                if end < 0:
                    return segments
                segments.append(_ToolCallSegment(self._buffer[:end]))
                self._buffer = self._buffer[end + len(_TOOL_CALL_CLOSE) :]
                self._in_call = False
                continue

            call_start = self._buffer.find(_TOOL_CALL_OPEN)
            result_start = self._buffer.find(_TOOL_RESULT_MARKER)
            if result_start >= 0 and (call_start < 0 or result_start < call_start):
                if result_start > 0:
                    segments.append(_TextSegment(self._buffer[:result_start]))
                self._buffer = ""
                segments.append(_FabricatedResultSegment())
                return segments
            if call_start >= 0:
                if call_start > 0:
                    segments.append(_TextSegment(self._buffer[:call_start]))
                self._buffer = self._buffer[call_start + len(_TOOL_CALL_OPEN) :]
                self._in_call = True
                continue

            hold = _held_back_suffix_len(
                self._buffer, (_TOOL_CALL_OPEN, _TOOL_RESULT_MARKER)
            )
            emit = self._buffer[: len(self._buffer) - hold]
            self._buffer = self._buffer[len(emit) :]
            if emit:
                segments.append(_TextSegment(emit))
            return segments

    def finish(self) -> list[_Segment]:
        if self._in_call:
            raise ValueError(
                f"Model output ended inside an unterminated {_TOOL_CALL_OPEN} block"
            )
        rest, self._buffer = self._buffer, ""
        return [_TextSegment(rest)] if rest else []


class _EventBuilder:
    """Turns parser segments into Anthropic content-block stream events."""

    def __init__(self, tool_names: set[str]) -> None:
        self._tool_names = tool_names
        self._next_index = 0
        self._open_text_index: int | None = None
        self._pending_whitespace = ""
        self.tool_call_count = 0

    def text(self, text: str) -> list[MessageStreamEvent]:
        # Whitespace between tool calls must not create empty text blocks.
        if self._open_text_index is None and not text.strip():
            self._pending_whitespace += text
            return []
        events: list[MessageStreamEvent] = []
        if self._open_text_index is None:
            self._open_text_index = self._next_index
            self._next_index += 1
            events.append(
                RawContentBlockStartEvent(
                    type="content_block_start",
                    index=self._open_text_index,
                    content_block=TextBlock(type="text", text=""),
                )
            )
            text = self._pending_whitespace + text
            self._pending_whitespace = ""
        events.append(
            RawContentBlockDeltaEvent(
                type="content_block_delta",
                index=self._open_text_index,
                delta=TextDelta(type="text_delta", text=text),
            )
        )
        return events

    def tool_call(self, raw: str) -> list[MessageStreamEvent]:
        try:
            payload = _ToolCallPayload.model_validate_json(raw.strip())
        except ValidationError as e:
            raise ValueError(f"Model wrote a malformed tool call: {raw.strip()}") from e
        if payload.name not in self._tool_names:
            raise ValueError(f"Model called unknown tool '{payload.name}'")

        events = self.close_text()
        self._pending_whitespace = ""
        index = self._next_index
        self._next_index += 1
        self.tool_call_count += 1
        events.extend(
            [
                RawContentBlockStartEvent(
                    type="content_block_start",
                    index=index,
                    content_block=ToolUseBlock(
                        type="tool_use",
                        id=f"toolu_cli_{uuid.uuid4().hex}",
                        name=payload.name,
                        input={},
                    ),
                ),
                RawContentBlockDeltaEvent(
                    type="content_block_delta",
                    index=index,
                    delta=InputJSONDelta(
                        type="input_json_delta",
                        partial_json=json.dumps(payload.input),
                    ),
                ),
                RawContentBlockStopEvent(type="content_block_stop", index=index),
            ]
        )
        return events

    def close_text(self) -> list[MessageStreamEvent]:
        if self._open_text_index is None:
            return []
        index, self._open_text_index = self._open_text_index, None
        return [RawContentBlockStopEvent(type="content_block_stop", index=index)]


def _render_tool_spec(tool: dict[str, Any]) -> str:
    return json.dumps(
        {
            "name": tool["name"],
            "description": tool.get("description"),
            "input_schema": tool["input_schema"],
        },
        ensure_ascii=False,
    )


class _TurnContentBuilder:
    """Accumulates a single CLI user turn, merging adjacent text."""

    def __init__(self) -> None:
        self.blocks: list[ContentBlockParam] = []
        self._text: list[str] = []

    def text(self, text: str) -> None:
        self._text.append(text)

    def block(self, block: ContentBlockParam) -> None:
        self._flush()
        self.blocks.append(block)

    def build(self) -> list[ContentBlockParam]:
        self._flush()
        return self.blocks

    def _flush(self) -> None:
        if self._text:
            self.blocks.append({"type": "text", "text": "".join(self._text)})
            self._text = []


def _append_block(builder: _TurnContentBuilder, block: ContentBlockParam) -> None:
    block_type = block["type"]
    if block_type == "text":
        builder.text(block["text"])
    elif block_type == "image":
        builder.block(block)
    elif block_type == "document":
        text = extract_text_document(block)
        if text is not None:
            builder.text(text)
        else:
            builder.block(block)
    elif block_type == "search_result":
        attrs = f"title={json.dumps(block['title'])} source={json.dumps(block['source'])}"
        builder.text(f"<search_result {attrs}>\n")
        for item in block["content"]:
            builder.text(item["text"])
            builder.text("\n")
        builder.text("</search_result>")
    elif block_type == "tool_use":
        call = {"name": block["name"], "input": block["input"]}
        builder.text(
            f'<tool_call id="{block["id"]}">{json.dumps(call, ensure_ascii=False)}</tool_call>'
        )
    elif block_type == "tool_result":
        error_attr = ' error="true"' if block.get("is_error") else ""
        builder.text(f'<tool_result id="{block["tool_use_id"]}"{error_attr}>\n')
        content = block.get("content")
        if isinstance(content, str):
            builder.text(content)
        elif content is not None:
            for item in content:
                _append_block(builder, cast(ContentBlockParam, item))
                builder.text("\n")
        builder.text("\n</tool_result>")
    elif block_type in ("thinking", "redacted_thinking"):
        # Thinking from earlier turns is not replayable through the CLI.
        return
    else:
        raise ValueError(f"Unsupported content block type for Claude CLI: {block_type}")


def build_cli_turn(
    messages: list[MessageParam] | None, prompt: str
) -> tuple[list[ContentBlockParam], bool]:
    """Build the single user turn sent to the CLI.

    Returns the content blocks and whether they hold a serialized transcript
    (which the system prompt then has to explain).
    """
    if not messages:
        return [{"type": "text", "text": prompt}], False

    api_messages = build_messages_for_anthropic_api(messages)
    builder = _TurnContentBuilder()

    if len(api_messages) == 1 and api_messages[0]["role"] == "user":
        content = api_messages[0]["content"]
        if isinstance(content, str):
            builder.text(content)
        else:
            for block in content:
                _append_block(builder, cast(ContentBlockParam, block))
        return builder.build(), False

    for message in api_messages:
        builder.text(f'<turn role="{message["role"]}">\n')
        content = message["content"]
        if isinstance(content, str):
            builder.text(content)
        else:
            for block in content:
                _append_block(builder, cast(ContentBlockParam, block))
                builder.text("\n")
        builder.text("\n</turn>\n")
    return builder.build(), True


def _is_context_overflow(status_code: int | None, message: str) -> bool:
    return status_code == 413 or "prompt is too long" in message.lower()


class ClaudeCliProvider(LLMProvider):
    """Provider that runs models through the Claude Code CLI (``claude -p``)."""

    provider_type: ClassVar[ProviderType] = ProviderType.CLAUDE_CLI

    def __init__(
        self,
        model: str,
        auth_token: str | None = None,
        cli_path: str = "claude",
    ):
        self.model = model
        self.model_name = model
        self._auth_token = auth_token
        self._cli_path = cli_path

    def _error(
        self,
        message: str,
        *,
        model: str,
        status_code: int | None = None,
        cause: BaseException | None = None,
    ) -> ProviderError:
        return ProviderError(
            message,
            provider_type=self.provider_type,
            model=model,
            status_code=status_code,
            cause=cause,
            is_context_overflow=_is_context_overflow(status_code, message),
        )

    def _build_env(self, max_tokens: int | None) -> dict[str, str]:
        env = dict(os.environ)
        env["DISABLE_AUTOUPDATER"] = "1"
        if max_tokens is not None:
            env["CLAUDE_CODE_MAX_OUTPUT_TOKENS"] = str(max_tokens)
        if self._auth_token:
            if self._auth_token.startswith(OAUTH_TOKEN_PREFIX):
                env.pop("ANTHROPIC_API_KEY", None)
                env["CLAUDE_CODE_OAUTH_TOKEN"] = self._auth_token
            else:
                env.pop("CLAUDE_CODE_OAUTH_TOKEN", None)
                env["ANTHROPIC_API_KEY"] = self._auth_token
        return env

    async def _run_cli(
        self,
        *,
        model: str,
        system_prompt: str,
        content: list[ContentBlockParam],
        max_tokens: int | None,
    ) -> AsyncIterator[RawMessageStreamEvent]:
        """Run one CLI invocation and yield the raw API stream events it reports."""
        with tempfile.TemporaryDirectory(prefix="omni-claude-cli-") as workdir:
            system_prompt_file = Path(workdir) / "system-prompt.md"
            system_prompt_file.write_text(system_prompt, encoding="utf-8")
            args = [
                "-p",
                "--input-format", "stream-json",
                "--output-format", "stream-json",
                "--include-partial-messages",
                "--verbose",
                "--tools", "",
                "--strict-mcp-config",
                "--setting-sources", "",
                "--no-session-persistence",
                "--max-turns", "1",
                "--model", model,
                "--system-prompt-file", str(system_prompt_file),
            ]
            try:
                proc = await asyncio.create_subprocess_exec(
                    self._cli_path,
                    *args,
                    stdin=asyncio.subprocess.PIPE,
                    stdout=asyncio.subprocess.PIPE,
                    stderr=asyncio.subprocess.PIPE,
                    cwd=workdir,
                    env=self._build_env(max_tokens),
                    limit=_STDOUT_LINE_LIMIT,
                )
            except FileNotFoundError as e:
                raise self._error(
                    f"Claude Code CLI not found at '{self._cli_path}'. Install it in the "
                    "omni-ai container (build with INSTALL_CLAUDE_CLI=true) or set CLAUDE_CLI_PATH.",
                    model=model,
                    cause=e,
                ) from e

            assert proc.stdin and proc.stdout and proc.stderr
            stderr_task = asyncio.create_task(proc.stderr.read())
            try:
                user_turn = {
                    "type": "user",
                    "message": {"role": "user", "content": content},
                }
                proc.stdin.write(json.dumps(user_turn).encode("utf-8") + b"\n")
                await proc.stdin.drain()
                proc.stdin.close()

                result: _CliResultLine | None = None
                async for raw_line in proc.stdout:
                    line = raw_line.decode("utf-8").strip()
                    if not line:
                        continue
                    try:
                        payload = json.loads(line)
                    except json.JSONDecodeError:
                        logger.warning("Ignoring non-JSON Claude CLI output: %s", line[:200])
                        continue
                    line_type = payload.get("type")
                    if line_type == "stream_event":
                        yield _RAW_EVENT_ADAPTER.validate_python(payload["event"])
                    elif line_type == "result":
                        result = _CliResultLine.model_validate(payload)

                return_code = await proc.wait()
                if result is None:
                    stderr = (await stderr_task).decode("utf-8", errors="replace")
                    raise self._error(
                        f"Claude CLI exited with code {return_code} without a result: "
                        f"{stderr[-_STDERR_TAIL_CHARS:].strip()}",
                        model=model,
                    )
                if result.is_error:
                    raise self._error(
                        result.result or "Claude CLI reported an error",
                        model=model,
                        status_code=result.api_error_status,
                    )
            finally:
                if proc.returncode is None:
                    proc.kill()
                    await proc.wait()
                stderr_task.cancel()

    async def stream_response(
        self,
        prompt: str,
        max_tokens: int | None = None,
        tools: list[dict[str, Any]] | None = None,
        messages: list[dict[str, Any]] | None = None,
        system_prompt: str | None = None,
        *,
        model: str | None = None,
    ) -> AsyncIterator[MessageStreamEvent]:
        active_model = model or self.model
        try:
            content, is_transcript = build_cli_turn(
                cast(list[MessageParam] | None, messages), prompt
            )
            full_system_prompt = system_prompt or ""
            if tools:
                specs = "\n".join(_render_tool_spec(t) for t in tools)
                full_system_prompt += _TOOLS_INSTRUCTIONS.format(tool_specs=specs)
            if is_transcript:
                full_system_prompt += _TRANSCRIPT_INSTRUCTIONS

            parser = ToolCallStreamParser()
            builder = _EventBuilder({t["name"] for t in tools or []})
            final_delta: RawMessageDeltaEvent | None = None
            fabricated_result = False
            message_started = False

            cli_events = self._run_cli(
                model=active_model,
                system_prompt=full_system_prompt,
                content=content,
                max_tokens=max_tokens,
            )
            # aclosing() makes an early break kill the subprocess right away.
            async with aclosing(cli_events):
                async for event in cli_events:
                    if event.type == "message_start":
                        if not message_started:
                            message_started = True
                            yield event
                        continue
                    if event.type == "message_delta":
                        final_delta = event
                        continue
                    text: str | None = None
                    if event.type == "content_block_start" and event.content_block.type == "text":
                        text = event.content_block.text
                    elif event.type == "content_block_delta" and event.delta.type == "text_delta":
                        text = event.delta.text
                    if not text:
                        continue
                    for segment in parser.feed(text):
                        if isinstance(segment, _FabricatedResultSegment):
                            fabricated_result = True
                            break
                        for out in self._segment_events(builder, segment):
                            yield out
                    if fabricated_result:
                        logger.warning(
                            "Claude CLI model invented a tool result; truncating its output"
                        )
                        break

            if not message_started:
                raise self._error("Claude CLI produced no model output", model=active_model)

            if not fabricated_result:
                for segment in parser.finish():
                    for out in self._segment_events(builder, segment):
                        yield out
            for out in builder.close_text():
                yield out

            stop_reason = "tool_use" if builder.tool_call_count else (
                final_delta.delta.stop_reason if final_delta else "end_turn"
            )
            usage = (
                final_delta.usage
                if final_delta
                else MessageDeltaUsage(output_tokens=0)
            )
            yield RawMessageDeltaEvent(
                type="message_delta",
                delta=Delta(stop_reason=stop_reason, stop_sequence=None),
                usage=usage,
            )
            yield RawMessageStopEvent(type="message_stop")
        except ProviderError:
            raise
        except Exception as e:
            logger.error("Claude CLI stream failed: %s", e, exc_info=True)
            raise self._error(str(e), model=active_model, cause=e) from e

    @staticmethod
    def _segment_events(
        builder: _EventBuilder, segment: _TextSegment | _ToolCallSegment
    ) -> list[MessageStreamEvent]:
        if isinstance(segment, _TextSegment):
            return builder.text(segment.text)
        return builder.tool_call(segment.raw)

    async def generate_response(
        self,
        prompt: str,
        max_tokens: int | None = None,
        *,
        model: str | None = None,
    ) -> tuple[str, TokenUsage]:
        parts: list[str] = []
        usage = TokenUsage()
        async for event in self.stream_response(prompt, max_tokens, model=model):
            if event.type == "message_start":
                start_usage = event.message.usage
                usage.input_tokens = start_usage.input_tokens
                usage.cache_read_tokens = start_usage.cache_read_input_tokens or 0
                usage.cache_creation_tokens = start_usage.cache_creation_input_tokens or 0
            elif event.type == "content_block_delta" and event.delta.type == "text_delta":
                parts.append(event.delta.text)
            elif event.type == "message_delta":
                usage.output_tokens = event.usage.output_tokens
                if event.usage.input_tokens is not None:
                    usage.input_tokens = event.usage.input_tokens
        return "".join(parts), usage

    async def verify_connection(self, *, model: str | None = None) -> None:
        """Run a minimal request, raising ProviderError when the CLI cannot answer."""
        await self.generate_response("Reply with the single word OK.", model=model)

    async def health_check(
        self,
        *,
        model: str | None = None,
    ) -> bool:
        try:
            await self.verify_connection(model=model)
            return True
        except ProviderError:
            return False
