"""Citation handling for LLM responses.

Handles both native Anthropic citations (convert_citation_to_param) and
synthetic citations for non-Anthropic providers (build/extract/emit).
"""

import re
from dataclasses import dataclass
from typing import cast

from anthropic.types import (
    MessageParam,
    TextBlockParam,
    TextCitationParam,
    CitationCharLocationParam,
    CitationPageLocationParam,
    CitationContentBlockLocationParam,
    CitationSearchResultLocationParam,
    CitationWebSearchResultLocationParam,
    CitationsDelta,
    CitationsSearchResultLocation,
    CitationCharLocation,
    DocumentBlockParam,
    PlainTextSourceParam,
    RawContentBlockDeltaEvent,
    SearchResultBlockParam,
    ToolResultBlockParam,
)

# ---------------------------------------------------------------------------
# Native Anthropic citation conversion
# ---------------------------------------------------------------------------


def convert_citation_to_param(citation_delta: CitationsDelta) -> TextCitationParam:
    """Convert a streaming CitationsDelta event to a persistable TextCitationParam."""
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


# ---------------------------------------------------------------------------
# Synthetic citations for non-Anthropic providers
# ---------------------------------------------------------------------------

_CITATION_PATTERN = re.compile(r"\[citation:(\d{1,3})\]")

CITATION_INSTRUCTION = (
    "\n\n# Citing sources\n"
    "When referencing information from search results or documents, you MUST cite the source "
    "using the exact format [citation:n] where n is the source number. For example: "
    '"The quarterly revenue increased by 15% [citation:1] while expenses decreased [citation:3]." '
    "Always place the citation immediately after the relevant claim."
)


@dataclass
class CitableRef:
    """Tracks a citable content block (search result or document) for synthetic citation generation."""

    index: int
    title: str
    source: str
    cited_text: str
    ref_type: str  # "search_result" or "document"


def _extract_text_from_search_result(block: SearchResultBlockParam) -> str:
    """Join text content blocks inside a search result."""
    return "\n".join(tb["text"] for tb in block["content"] if tb["type"] == "text")


def _extract_data_from_document(block: DocumentBlockParam) -> str:
    """Extract the plain-text data from a document block's source, if available."""
    source = block["source"]
    if source["type"] == "text":
        return cast(PlainTextSourceParam, source)["data"]
    return ""


def build_citable_index(
    messages: list[MessageParam],
) -> dict[int, CitableRef]:
    """Scan all messages for search_result and document blocks in tool results.

    Returns a 1-based index mapping to CitableRef metadata.
    """
    index_map: dict[int, CitableRef] = {}
    counter = 1
    for msg in messages:
        content = msg["content"]
        if isinstance(content, str):
            continue
        for block in content:
            if not isinstance(block, dict) or block.get("type") != "tool_result":
                continue
            tool_block = cast(ToolResultBlockParam, block)
            tool_content = tool_block.get("content", [])
            if isinstance(tool_content, str):
                continue
            for sub_block in tool_content:
                if not isinstance(sub_block, dict):
                    continue

                if sub_block.get("type") == "search_result":
                    sr = cast(SearchResultBlockParam, sub_block)
                    index_map[counter] = CitableRef(
                        index=counter,
                        title=sr["title"],
                        source=sr["source"],
                        cited_text=_extract_text_from_search_result(sr)[:500],
                        ref_type="search_result",
                    )
                    counter += 1

                elif sub_block.get("type") == "document":
                    doc = cast(DocumentBlockParam, sub_block)
                    index_map[counter] = CitableRef(
                        index=counter,
                        title=doc.get("title", ""),
                        source=doc.get("title", ""),
                        cited_text=_extract_data_from_document(doc)[:500],
                        ref_type="document",
                    )
                    counter += 1

    return index_map


def _strip_citations(block: TextBlockParam) -> TextBlockParam:
    """Return a text block without the citations field."""
    if "citations" not in block:
        return block
    return TextBlockParam(type="text", text=block["text"])


def prepare_messages_for_non_citation_provider(
    messages: list[MessageParam],
    citable_index: dict[int, CitableRef],
) -> list[MessageParam]:
    """Create a copy of messages with search_result/document blocks replaced by numbered text
    and citations stripped from assistant text blocks.

    Does not mutate the original messages.
    """
    if not citable_index:
        return messages

    transformed: list[MessageParam] = []
    counter = 1
    for msg in messages:
        content = msg["content"]
        if isinstance(content, str):
            transformed.append(msg)
            continue

        new_content = []
        has_changes = False
        for block in content:
            if not isinstance(block, dict):
                new_content.append(block)
                continue

            block_type = block["type"]

            # Strip citations from text blocks (prior assistant responses)
            if block_type == "text":
                text_block = cast(TextBlockParam, block)
                if "citations" in text_block:
                    has_changes = True
                    new_content.append(_strip_citations(text_block))
                else:
                    new_content.append(block)
                continue

            if block_type != "tool_result":
                new_content.append(block)
                continue

            tool_block = cast(ToolResultBlockParam, block)
            tool_content = tool_block.get("content", [])
            if isinstance(tool_content, str):
                new_content.append(block)
                continue

            has_citable = any(
                isinstance(sb, dict) and sb["type"] in ("search_result", "document")
                for sb in tool_content
            )

            if not has_citable:
                new_content.append(block)
                continue

            has_changes = True
            new_sub_blocks: list[TextBlockParam] = []
            for sb in tool_content:
                if not isinstance(sb, dict):
                    continue

                sb_type = sb["type"]
                if sb_type == "search_result":
                    sr = cast(SearchResultBlockParam, sb)
                    ref = citable_index.get(counter)
                    if ref:
                        inner_text = _extract_text_from_search_result(sr)
                        new_sub_blocks.append(
                            TextBlockParam(
                                type="text",
                                text=f"[{counter}] {ref.title} ({ref.source})\n{inner_text}",
                            )
                        )
                    counter += 1

                elif sb_type == "document":
                    doc = cast(DocumentBlockParam, sb)
                    ref = citable_index.get(counter)
                    if ref:
                        data = _extract_data_from_document(doc)
                        new_sub_blocks.append(
                            TextBlockParam(
                                type="text",
                                text=f"[{counter}] Document: {ref.title}\n{data}",
                            )
                        )
                    counter += 1

                else:
                    new_sub_blocks.append(cast(TextBlockParam, sb))

            new_block: ToolResultBlockParam = {
                **tool_block,
                "content": new_sub_blocks,
            }
            new_content.append(new_block)

        if has_changes:
            transformed.append(MessageParam(role=msg["role"], content=new_content))
        else:
            transformed.append(msg)

    return transformed


def extract_synthetic_citations(
    text: str,
    citable_index: dict[int, CitableRef],
) -> tuple[str, list[TextCitationParam]]:
    """Extract [citation:n] references from text and create synthetic citation objects.

    Returns (cleaned_text, citations) where cleaned_text has markers stripped.
    """
    citations: list[TextCitationParam] = []
    seen: set[int] = set()

    for match in _CITATION_PATTERN.finditer(text):
        ref_num = int(match.group(1))
        if ref_num in seen:
            continue
        seen.add(ref_num)

        ref = citable_index.get(ref_num)
        if ref is None:
            continue

        if ref.ref_type == "search_result":
            citations.append(
                CitationSearchResultLocationParam(
                    type="search_result_location",
                    start_block_index=0,
                    end_block_index=0,
                    search_result_index=ref.index - 1,  # 0-based
                    title=ref.title,
                    source=ref.source,
                    cited_text=ref.cited_text[:200],
                )
            )
        elif ref.ref_type == "document":
            citations.append(
                CitationCharLocationParam(
                    type="char_location",
                    document_index=ref.index - 1,  # 0-based
                    document_title=ref.title,
                    start_char_index=0,
                    end_char_index=0,
                    cited_text=ref.cited_text[:200],
                )
            )

    cleaned_text = _CITATION_PATTERN.sub("", text) if citations else text
    return cleaned_text, citations


def build_synthetic_citation_event(
    block_idx: int,
    citation_param: TextCitationParam,
) -> RawContentBlockDeltaEvent:
    """Build an Anthropic-compatible citation delta event from a synthetic citation."""
    param = cast(dict, citation_param)
    if param["type"] == "search_result_location":
        citation_obj = CitationsSearchResultLocation(
            type="search_result_location",
            search_result_index=param["search_result_index"],
            start_block_index=param["start_block_index"],
            end_block_index=param["end_block_index"],
            title=param["title"],
            source=param["source"],
            cited_text=param["cited_text"],
        )
    else:
        citation_obj = CitationCharLocation(
            type="char_location",
            document_index=param["document_index"],
            document_title=param.get("document_title"),
            start_char_index=param["start_char_index"],
            end_char_index=param["end_char_index"],
            cited_text=param["cited_text"],
        )

    return RawContentBlockDeltaEvent(
        type="content_block_delta",
        index=block_idx,
        delta=CitationsDelta(type="citations_delta", citation=citation_obj),
    )
