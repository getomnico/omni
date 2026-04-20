import pytest
from types import SimpleNamespace
from anthropic.types import MessageParam, ToolUseBlockParam, ToolResultBlockParam

from agent_loop import (
    AgentLoopResult,
    AssistantMessageComplete,
    LLMStreamEvent,
    LoopComplete,
    ToolApprovalRequired,
    ToolResultEvent,
    ToolResultMessageComplete,
    run_agent_loop,
)
from tools import ToolContext, ToolRegistry
from tools.registry import ToolHandler, ToolResult


def test_event_types_are_constructible():
    msg = MessageParam(role="assistant", content=[])
    tool_call = ToolUseBlockParam(type="tool_use", id="t1", name="search_documents", input={})
    tool_result = ToolResultBlockParam(type="tool_result", tool_use_id="t1", content=[])
    user_msg = MessageParam(role="user", content=[tool_result])
    result = AgentLoopResult(final_messages=[msg], iterations=1, stopped_reason="no_tool_calls")

    events = [
        LLMStreamEvent(raw_event=object()),
        AssistantMessageComplete(message=msg),
        ToolApprovalRequired(tool_call=tool_call),
        ToolResultEvent(tool_result=tool_result),
        ToolResultMessageComplete(message=user_msg),
        LoopComplete(result=result),
    ]
    assert len(events) == 6
    assert result.stopped_reason == "no_tool_calls"


class _FakeStream:
    """Mimics anthropic.AsyncStream by being an async iterator."""

    def __init__(self, events):
        self._events = events

    def __aiter__(self):
        async def gen():
            for e in self._events:
                yield e
        return gen()


class _FakeProvider:
    """Two-iteration script: first emits a tool_use; second emits text only."""

    def __init__(self):
        self.call_count = 0

    def stream_response(self, **kwargs):
        self.call_count += 1
        if self.call_count == 1:
            events = [
                SimpleNamespace(type="message_start"),
                SimpleNamespace(
                    type="content_block_start",
                    index=0,
                    content_block=SimpleNamespace(
                        type="tool_use", id="tool_1", name="search_documents", input={}
                    ),
                ),
                SimpleNamespace(
                    type="content_block_delta",
                    index=0,
                    delta=SimpleNamespace(
                        type="input_json_delta",
                        partial_json='{"query": "hello"}',
                    ),
                ),
                SimpleNamespace(type="message_stop"),
            ]
        else:
            events = [
                SimpleNamespace(type="message_start"),
                SimpleNamespace(
                    type="content_block_start",
                    index=0,
                    content_block=SimpleNamespace(type="text", text=""),
                ),
                SimpleNamespace(
                    type="content_block_delta",
                    index=0,
                    delta=SimpleNamespace(type="text_delta", text="Final answer."),
                ),
                SimpleNamespace(type="message_stop"),
            ]
        for e in events:
            e.to_json = lambda *a, **k: "{}"
        return _FakeStream(events)


class _FakeSearchHandler(ToolHandler):
    def get_tools(self):
        return [{"name": "search_documents"}]
    def can_handle(self, name):
        return name == "search_documents"
    async def execute(self, name, tool_input, context):
        return ToolResult(content=[{"type": "text", "text": "chunk-A"}], is_error=False)
    def requires_approval(self, name):
        return False


@pytest.mark.asyncio
async def test_run_agent_loop_executes_one_tool_then_completes():
    registry = ToolRegistry()
    registry.register(_FakeSearchHandler())
    provider = _FakeProvider()
    ctx = ToolContext(chat_id="c1", user_id="u1", skip_permission_check=True)

    events = []
    async for ev in run_agent_loop(
        llm_provider=provider,
        messages=[MessageParam(role="user", content="Q?")],
        system_prompt="sys",
        tools=[{"name": "search_documents"}],
        registry=registry,
        tool_context=ctx,
        max_iterations=5,
        max_tokens=512,
        temperature=0.0,
        top_p=1.0,
    ):
        events.append(ev)

    assert provider.call_count == 2
    assert isinstance(events[-1], LoopComplete)
    assert events[-1].result.stopped_reason == "no_tool_calls"
    assert events[-1].result.iterations == 2
    kinds = [type(e).__name__ for e in events]
    assert "ToolResultEvent" in kinds
    assert "ToolResultMessageComplete" in kinds
    assert "AssistantMessageComplete" in kinds
