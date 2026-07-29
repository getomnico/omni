"""Adapters for Omni's Anthropic-shaped internal messages.

Omni stores a small number of internal-only fields on Anthropic-compatible blocks
so the UI and downstream logic can retain source metadata. Anthropic-family APIs
reject those extras, so providers must pass messages through this adapter before
sending requests.
"""

import json
from collections.abc import Iterable
from typing import cast

from anthropic.types import (
    ContentBlockParam,
    DocumentBlockParam,
    MessageParam,
    SearchResultBlockParam,
    ToolResultBlockParam,
)
from anthropic.types.tool_result_block_param import (
    Content as ToolResultContentBlockParam,
)


class OmniSearchResultBlockParam(SearchResultBlockParam, total=False):
    source_type: str


type OmniContentBlockParam = ContentBlockParam | OmniSearchResultBlockParam
type OmniToolResultContentBlockParam = (
    ToolResultContentBlockParam | OmniSearchResultBlockParam
)
type AnthropicMessageContent = str | Iterable[OmniContentBlockParam]


def extract_text_document(block: DocumentBlockParam) -> str | None:
    """Convert a text-backed document block into provider-neutral text."""
    if block.get("type") != "document":
        return None

    source = block.get("source")
    if not isinstance(source, dict) or source.get("type") != "text":
        return None

    data = source.get("data")
    if not isinstance(data, str) or not data:
        return None

    title = block.get("title")
    if isinstance(title, str) and title:
        return f"Document title: {json.dumps(title, ensure_ascii=False)}\nDocument content:\n{data}"
    return f"Document content:\n{data}"


def build_messages_for_anthropic_api(
    messages: list[MessageParam],
) -> list[MessageParam]:
    """Convert Omni's internal message blocks to Anthropic's request shape."""
    return [
        MessageParam(
            role=msg["role"],
            content=_build_content_for_api(cast(AnthropicMessageContent, msg["content"])),
        )
        for msg in messages
    ]


def _build_content_for_api(
    content: AnthropicMessageContent,
) -> str | list[ContentBlockParam]:
    if isinstance(content, str):
        return content
    return [_build_block_for_api(block) for block in content]


def _build_block_for_api(block: OmniContentBlockParam) -> ContentBlockParam:
    if block["type"] == "tool_result":
        return _build_tool_result_block_for_api(cast(ToolResultBlockParam, block))
    if block["type"] == "search_result":
        return _build_search_result_block_for_api(
            cast(OmniSearchResultBlockParam, block)
        )
    return block


def _build_tool_result_block_for_api(
    block: ToolResultBlockParam,
) -> ToolResultBlockParam:
    result = ToolResultBlockParam(
        type="tool_result",
        tool_use_id=block["tool_use_id"],
    )
    if "content" in block:
        result["content"] = _build_tool_result_content_for_api(block["content"])
    if "is_error" in block:
        result["is_error"] = block["is_error"]
    if "cache_control" in block:
        result["cache_control"] = block["cache_control"]
    return result


def _build_tool_result_content_for_api(
    content: str | Iterable[OmniToolResultContentBlockParam],
) -> str | list[ToolResultContentBlockParam]:
    if isinstance(content, str):
        return content
    return [_build_tool_result_content_block_for_api(block) for block in content]


def _build_tool_result_content_block_for_api(
    block: OmniToolResultContentBlockParam,
) -> ToolResultContentBlockParam:
    if block["type"] == "search_result":
        return _build_search_result_block_for_api(cast(OmniSearchResultBlockParam, block))
    return block


def _build_search_result_block_for_api(
    block: OmniSearchResultBlockParam,
) -> SearchResultBlockParam:
    result = SearchResultBlockParam(
        type="search_result",
        title=block["title"],
        source=block["source"],
        content=block["content"],
    )
    if "citations" in block:
        result["citations"] = block["citations"]
    if "cache_control" in block:
        result["cache_control"] = block["cache_control"]
    return result
