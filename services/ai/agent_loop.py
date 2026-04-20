"""Pure, transport-agnostic chat agent loop.

Shared between the production chat router (services/ai/routers/chat.py),
which converts emitted events into SSE for the browser, and the eval
runner, which consumes the same events headlessly to score the loop.

The loop emits typed events via an async generator. The terminal event
is always LoopComplete(result=AgentLoopResult).
"""

from __future__ import annotations

import json
import logging
from collections.abc import AsyncIterator, Awaitable, Callable
from dataclasses import dataclass
from typing import Literal, Union, cast

from anthropic import AsyncStream, MessageStreamEvent
from anthropic.types import (
    CitationCharLocationParam,
    CitationContentBlockLocationParam,
    CitationPageLocationParam,
    CitationSearchResultLocationParam,
    CitationWebSearchResultLocationParam,
    CitationsDelta,
    MessageParam,
    TextBlockParam,
    TextCitationParam,
    ToolResultBlockParam,
    ToolUseBlockParam,
)

from providers import LLMProvider
from tools import ToolContext, ToolRegistry

logger = logging.getLogger(__name__)


@dataclass
class LLMStreamEvent:
    """Pass-through Anthropic stream event. Caller owns serialization."""
    raw_event: MessageStreamEvent


@dataclass
class AssistantMessageComplete:
    """Emitted once an assistant turn is fully assembled (after message_stop)."""
    message: MessageParam


@dataclass
class ToolApprovalRequired:
    """A tool call needs approval. The loop returns immediately after emitting
    this event; the caller is responsible for persisting state and resuming."""
    tool_call: ToolUseBlockParam


@dataclass
class ToolResultEvent:
    """A single tool_result block produced by the registry."""
    tool_result: ToolResultBlockParam


@dataclass
class ToolResultMessageComplete:
    """Emitted once all tool_results for an iteration are bundled into a user message."""
    message: MessageParam


@dataclass
class AgentLoopResult:
    """Final state returned with LoopComplete."""
    final_messages: list[MessageParam]
    iterations: int
    stopped_reason: str  # "no_tool_calls" | "max_iterations" | "approval_required" | "stopped" | "error"
    error: str | None = None


@dataclass
class LoopComplete:
    """Terminal event. Always last."""
    result: AgentLoopResult


AgentLoopEvent = Union[
    LLMStreamEvent,
    AssistantMessageComplete,
    ToolApprovalRequired,
    ToolResultEvent,
    ToolResultMessageComplete,
    LoopComplete,
]


WrapStream = Callable[[AsyncStream[MessageStreamEvent]], AsyncIterator[MessageStreamEvent]]
ShouldStop = Callable[[], Awaitable[bool]]
OnIterationEnd = Callable[[], Awaitable[None]]
ApprovalDecision = Literal["pause", "deny"]
ApprovalHandler = Callable[[ToolUseBlockParam], Awaitable[ApprovalDecision]]


def _convert_citation_to_param(citation_delta: CitationsDelta) -> TextCitationParam:
    """Convert an Anthropic CitationsDelta event into its param form."""
    citation = citation_delta.citation
    if citation.type == "char_location":
        return CitationCharLocationParam(
            type="char_location",
            start_char_index=citation.start_char_index,
            end_char_index=citation.end_char_index,
            document_title=citation.document_title,
            document_index=citation.document_index,
            cited_text=citation.cited_text,
        )
    elif citation.type == "page_location":
        return CitationPageLocationParam(
            type="page_location",
            start_page_number=citation.start_page_number,
            end_page_number=citation.end_page_number,
            document_title=citation.document_title,
            document_index=citation.document_index,
            cited_text=citation.cited_text,
        )
    elif citation.type == "content_block_location":
        return CitationContentBlockLocationParam(
            type="content_block_location",
            start_block_index=citation.start_block_index,
            end_block_index=citation.end_block_index,
            document_title=citation.document_title,
            document_index=citation.document_index,
            cited_text=citation.cited_text,
        )
    elif citation.type == "search_result_location":
        return CitationSearchResultLocationParam(
            type="search_result_location",
            start_block_index=citation.start_block_index,
            end_block_index=citation.end_block_index,
            search_result_index=citation.search_result_index,
            title=citation.title,
            source=citation.source,
            cited_text=citation.cited_text,
        )
    elif citation.type == "web_search_result_location":
        return CitationWebSearchResultLocationParam(
            type="web_search_result_location",
            url=citation.url,
            title=citation.title,
            encrypted_index=citation.encrypted_index,
            cited_text=citation.cited_text,
        )
    else:
        raise ValueError(f"Unknown citation type: {citation.type}")


async def run_agent_loop(
    *,
    llm_provider: LLMProvider,
    messages: list[MessageParam],
    system_prompt: str,
    tools: list[dict],
    registry: ToolRegistry,
    tool_context: ToolContext,
    max_iterations: int,
    max_tokens: int,
    temperature: float,
    top_p: float,
    wrap_stream: WrapStream | None = None,
    should_stop: ShouldStop | None = None,
    on_iteration_end: OnIterationEnd | None = None,
    approval_handler: ApprovalHandler | None = None,
) -> AsyncIterator[AgentLoopEvent]:
    """Run the agent tool-calling loop, yielding typed events.

    The caller is responsible for:
      - Translating LLMStreamEvent into its own transport (SSE for chat,
        accumulator for eval).
      - Persisting messages emitted via *MessageComplete events if needed.
      - Reacting to ToolApprovalRequired (the loop returns immediately
        after emitting it; resume is the caller's responsibility).
    """
    conversation_messages = list(messages)
    iterations_run = 0
    stopped_reason = "max_iterations"

    for iteration in range(max_iterations):
        iterations_run = iteration + 1

        if should_stop and await should_stop():
            stopped_reason = "stopped"
            break

        logger.info(f"agent_loop iteration {iterations_run}/{max_iterations}")
        content_blocks: list[TextBlockParam | ToolUseBlockParam] = []

        raw_stream = llm_provider.stream_response(
            prompt="",
            messages=conversation_messages,
            tools=tools,
            max_tokens=max_tokens,
            temperature=temperature,
            top_p=top_p,
            system_prompt=system_prompt,
        )
        stream = wrap_stream(raw_stream) if wrap_stream else raw_stream

        message_stop_received = False
        async for event in stream:
            if event.type == "content_block_start":
                if event.content_block.type == "text":
                    content_blocks.append(
                        TextBlockParam(type="text", text=event.content_block.text)
                    )
                elif event.content_block.type == "tool_use":
                    content_blocks.append(
                        ToolUseBlockParam(
                            type="tool_use",
                            id=event.content_block.id,
                            name=event.content_block.name,
                            input="",
                        )
                    )
            elif event.type == "content_block_delta":
                if event.delta.type == "text_delta":
                    if event.index >= len(content_blocks):
                        content_blocks.append(TextBlockParam(type="text", text=""))
                    blk = cast(TextBlockParam, content_blocks[event.index])
                    blk["text"] += event.delta.text
                elif event.delta.type == "input_json_delta":
                    if event.index >= len(content_blocks):
                        content_blocks.append(
                            ToolUseBlockParam(type="tool_use", id="", name="", input="")
                        )
                    tu = cast(ToolUseBlockParam, content_blocks[event.index])
                    tu["input"] = cast(str, tu["input"]) + event.delta.partial_json
                elif event.delta.type == "citations_delta":
                    if event.index >= len(content_blocks):
                        content_blocks.append(
                            TextBlockParam(type="text", text="", citations=[])
                        )
                    blk = cast(TextBlockParam, content_blocks[event.index])
                    if "citations" not in blk or not blk["citations"]:
                        blk["citations"] = []
                    citations = cast(list[TextCitationParam], blk["citations"])
                    citations.append(_convert_citation_to_param(event.delta))
            elif event.type == "message_stop":
                message_stop_received = True

            yield LLMStreamEvent(raw_event=event)

            if message_stop_received:
                break

        if on_iteration_end:
            await on_iteration_end()

        tool_calls = [b for b in content_blocks if b["type"] == "tool_use"]
        for tc in tool_calls:
            try:
                tc["input"] = json.loads(cast(str, tc["input"]))
            except json.JSONDecodeError as e:
                logger.error(f"Failed to parse tool_call input as JSON: {tc['input']}: {e}")
                tc["input"] = {}

        assistant_message = MessageParam(role="assistant", content=content_blocks)
        conversation_messages.append(assistant_message)
        yield AssistantMessageComplete(message=assistant_message)

        if not tool_calls:
            stopped_reason = "no_tool_calls"
            break

        if should_stop and await should_stop():
            stopped_reason = "stopped"
            break

        tool_results: list[ToolResultBlockParam] = []
        for tool_call in tool_calls:
            if registry.requires_approval(tool_call["name"]):
                decision: ApprovalDecision = (
                    await approval_handler(tool_call) if approval_handler else "pause"
                )
                if decision == "pause":
                    yield ToolApprovalRequired(tool_call=tool_call)
                    stopped_reason = "approval_required"
                    yield LoopComplete(
                        result=AgentLoopResult(
                            final_messages=conversation_messages,
                            iterations=iterations_run,
                            stopped_reason=stopped_reason,
                        )
                    )
                    return
                denied_result = ToolResultBlockParam(
                    type="tool_result",
                    tool_use_id=tool_call["id"],
                    content=[
                        {
                            "type": "text",
                            "text": "This action requires user approval, but the approval system is not available.",
                        }
                    ],
                    is_error=True,
                )
                tool_results.append(denied_result)
                continue

            result = await registry.execute(
                tool_call["name"], tool_call["input"], tool_context
            )
            tool_result = ToolResultBlockParam(
                type="tool_result",
                tool_use_id=tool_call["id"],
                content=result.content,
                is_error=result.is_error,
            )
            tool_results.append(tool_result)
            yield ToolResultEvent(tool_result=tool_result)

        tool_result_message = MessageParam(role="user", content=tool_results)
        conversation_messages.append(tool_result_message)
        yield ToolResultMessageComplete(message=tool_result_message)

    yield LoopComplete(
        result=AgentLoopResult(
            final_messages=conversation_messages,
            iterations=iterations_run,
            stopped_reason=stopped_reason,
        )
    )
