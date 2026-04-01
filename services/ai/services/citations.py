"""Citation handling for LLM responses.

Handles both native Anthropic citations and synthetic citations for
non-Anthropic providers via the CitationProcessor class.
"""

import re
from dataclasses import dataclass
from typing import cast

from services.stream import StreamProcessor

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
    TextDelta,
    DocumentBlockParam,
    PlainTextSourceParam,
    RawContentBlockDeltaEvent,
    SearchResultBlockParam,
    ToolResultBlockParam,
)

# Matches [citation:1], [citation:9, 3, 4], [citation:1,2], etc.
_CITATION_PATTERN = re.compile(r"\[citation:([\d,\s]+)\]")
_NUM_PATTERN = re.compile(r"\d+")

CITATION_INSTRUCTION = (
    "\n\n# Citing sources\n"
    "When referencing information from search results or documents, you MUST cite the source "
    "using the exact format [citation:n] where n is the source number. For example: "
    '"The quarterly revenue increased by 15% [citation:1] while expenses decreased [citation:3]." '
    "Always place the citation immediately after the relevant claim. "
    "For multiple citations, separate them with spaces: [citation:1] [citation:2]. "
    "You may also combine them: [citation:1, 2]."
)


@dataclass
class CitableRef:
    """Tracks a citable content block (search result or document) for synthetic citation generation."""

    index: int
    title: str
    source: str
    cited_text: str
    ref_type: str  # "search_result" or "document"


class CitationProcessor:
    """Handles citation conversion, indexing, and synthesis for LLM responses."""

    # ------------------------------------------------------------------
    # Native Anthropic citation conversion
    # ------------------------------------------------------------------

    @staticmethod
    def convert_delta_to_param(citation_delta: CitationsDelta) -> TextCitationParam:
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

    # ------------------------------------------------------------------
    # Citable content indexing
    # ------------------------------------------------------------------

    @staticmethod
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

    # ------------------------------------------------------------------
    # Message preparation for non-citation providers
    # ------------------------------------------------------------------

    @staticmethod
    def prepare_messages(
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

    # ------------------------------------------------------------------
    # Synthetic citation extraction
    # ------------------------------------------------------------------

    @staticmethod
    def extract_citations(
        text: str,
        citable_index: dict[int, CitableRef],
    ) -> tuple[str, list[TextCitationParam]]:
        """Extract [citation:n] references from text and create synthetic citation objects.

        Returns (cleaned_text, citations) where cleaned_text has markers stripped.
        """
        citations: list[TextCitationParam] = []
        seen: set[int] = set()

        for match in _CITATION_PATTERN.finditer(text):
            # Parse all numbers from the match (handles "1", "9, 3, 4", "1,2", etc.)
            ref_nums = [int(n) for n in _NUM_PATTERN.findall(match.group(1))]
            for ref_num in ref_nums:
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

    # ------------------------------------------------------------------
    # Synthetic citation event building
    # ------------------------------------------------------------------

    @staticmethod
    def build_event(
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


class CitationStreamProcessor(StreamProcessor):
    """StreamProcessor that strips [citation:...] markers from text deltas and emits
    synthetic citation_delta events inline.

    Buffers text when a potential citation marker start is detected. When a complete
    marker is found, it's swallowed and a citation_delta event is emitted instead.
    """

    _PREFIX = "[citation:"

    def __init__(self, citable_index: dict[int, CitableRef]) -> None:
        self._citable_index = citable_index
        self._buf = ""
        self._current_index = 0  # tracks the content block index for emitting events

    def process(
        self,
        event: "MessageStreamEvent",
    ) -> list["MessageStreamEvent"]:
        from anthropic.types.message_stream_event import MessageStreamEvent as _MSE

        if event.type == "content_block_delta" and event.delta.type == "text_delta":
            self._current_index = event.index
            return self._process_text_delta(event)

        if event.type == "content_block_stop":
            # Flush remaining buffer before the block closes
            flush_events = self._flush_buffer(event.index)
            return [*flush_events, event]

        return [event]

    def flush(self) -> list["MessageStreamEvent"]:
        return self._flush_buffer(self._current_index)

    # ------------------------------------------------------------------

    def _process_text_delta(
        self,
        event: "MessageStreamEvent",
    ) -> list["MessageStreamEvent"]:
        """Buffer text, emit clean text deltas and citation events."""
        self._buf += event.delta.text
        results: list["MessageStreamEvent"] = []

        clean_text, citation_markers = self._consume_buffer()

        if clean_text:
            results.append(
                RawContentBlockDeltaEvent(
                    type="content_block_delta",
                    index=event.index,
                    delta=TextDelta(type="text_delta", text=clean_text),
                )
            )

        for marker_text in citation_markers:
            ref_nums = [int(n) for n in _NUM_PATTERN.findall(marker_text)]
            for ref_num in ref_nums:
                citation_event = self._build_citation_event(event.index, ref_num)
                if citation_event:
                    results.append(citation_event)

        return results

    def _consume_buffer(self) -> tuple[str, list[str]]:
        """Parse the buffer, returning (clean_text, list_of_swallowed_marker_contents).

        Leaves any incomplete potential marker in self._buf.
        """
        out: list[str] = []
        markers: list[str] = []

        while self._buf:
            bracket = self._buf.find("[")
            if bracket == -1:
                out.append(self._buf)
                self._buf = ""
                break

            if bracket > 0:
                out.append(self._buf[:bracket])
                self._buf = self._buf[bracket:]

            prefix = self._PREFIX
            if len(self._buf) < len(prefix):
                if prefix.startswith(self._buf):
                    break  # could still become a citation, keep buffering
                else:
                    out.append(self._buf[0])
                    self._buf = self._buf[1:]
                    continue

            if not self._buf.startswith(prefix):
                out.append(self._buf[0])
                self._buf = self._buf[1:]
                continue

            close = self._buf.find("]", len(prefix))
            if close == -1:
                rest = self._buf[len(prefix) :]
                if all(c in "0123456789, " for c in rest):
                    break  # keep buffering
                else:
                    out.append(self._buf[0])
                    self._buf = self._buf[1:]
                    continue

            candidate = self._buf[: close + 1]
            m = _CITATION_PATTERN.match(candidate)
            if m:
                markers.append(m.group(1))
                self._buf = self._buf[close + 1 :]
                # Consume surrounding whitespace so "text [citation:1] more"
                # becomes "text more" instead of "text  more".
                # Strip space before the marker; the space after (if any)
                # naturally becomes the single separator.
                if out and out[-1].endswith(" "):
                    out[-1] = out[-1][:-1]
            else:
                out.append(self._buf[0])
                self._buf = self._buf[1:]

        return "".join(out), markers

    def _flush_buffer(self, block_index: int) -> list["MessageStreamEvent"]:
        """Emit any remaining buffered text as a text_delta event."""
        if not self._buf:
            return []
        text = self._buf
        self._buf = ""
        return [
            RawContentBlockDeltaEvent(
                type="content_block_delta",
                index=block_index,
                delta=TextDelta(type="text_delta", text=text),
            )
        ]

    def _build_citation_event(
        self, block_index: int, ref_num: int
    ) -> RawContentBlockDeltaEvent | None:
        ref = self._citable_index.get(ref_num)
        if ref is None:
            return None

        if ref.ref_type == "search_result":
            citation_obj = CitationsSearchResultLocation(
                type="search_result_location",
                search_result_index=ref.index - 1,
                start_block_index=0,
                end_block_index=0,
                title=ref.title,
                source=ref.source,
                cited_text=ref.cited_text[:200],
            )
        else:
            citation_obj = CitationCharLocation(
                type="char_location",
                document_index=ref.index - 1,
                document_title=ref.title,
                start_char_index=0,
                end_char_index=0,
                cited_text=ref.cited_text[:200],
            )

        return RawContentBlockDeltaEvent(
            type="content_block_delta",
            index=block_index,
            delta=CitationsDelta(type="citations_delta", citation=citation_obj),
        )


# ---------------------------------------------------------------------------
# Private helpers
# ---------------------------------------------------------------------------


def _extract_text_from_search_result(block: SearchResultBlockParam) -> str:
    """Join text content blocks inside a search result."""
    return "\n".join(tb["text"] for tb in block["content"] if tb["type"] == "text")


def _extract_data_from_document(block: DocumentBlockParam) -> str:
    """Extract the plain-text data from a document block's source, if available."""
    source = block["source"]
    if source["type"] == "text":
        return cast(PlainTextSourceParam, source)["data"]
    return ""


def _strip_citations(block: TextBlockParam) -> TextBlockParam:
    """Return a text block without the citations field."""
    if "citations" not in block:
        return block
    return TextBlockParam(type="text", text=block["text"])
