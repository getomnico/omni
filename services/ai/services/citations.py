"""Citation handling for LLM responses.

Handles both native Anthropic citations and synthetic citations for
non-Anthropic providers via the CitationProcessor class.
"""

import re
from collections.abc import AsyncIterator
from dataclasses import dataclass
from typing import cast

from anthropic.types import (
    CitationCharLocation,
    CitationCharLocationParam,
    CitationContentBlockLocationParam,
    CitationPageLocationParam,
    CitationsDelta,
    CitationSearchResultLocationParam,
    CitationsSearchResultLocation,
    CitationWebSearchResultLocationParam,
    DocumentBlockParam,
    MessageParam,
    MessageStreamEvent,
    PlainTextSourceParam,
    RawContentBlockDeltaEvent,
    RawContentBlockStartEvent,
    RawContentBlockStopEvent,
    SearchResultBlockParam,
    TextBlock,
    TextBlockParam,
    TextCitationParam,
    TextDelta,
    ToolResultBlockParam,
)

from streaming.processor import StreamProcessor

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
    citation_index: int
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
        search_result_counter = 0
        document_counter = 0
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
                            citation_index=search_result_counter,
                            title=sr["title"],
                            source=sr["source"],
                            cited_text=_extract_text_from_search_result(sr)[:500],
                            ref_type="search_result",
                        )
                        counter += 1
                        search_result_counter += 1

                    elif sub_block.get("type") == "document":
                        doc = cast(DocumentBlockParam, sub_block)
                        index_map[counter] = CitableRef(
                            index=counter,
                            citation_index=document_counter,
                            title=doc.get("title", ""),
                            source=doc.get("title", ""),
                            cited_text=_extract_data_from_document(doc)[:500],
                            ref_type="document",
                        )
                        counter += 1
                        document_counter += 1

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
                    isinstance(sb, dict) and sb.get("type") in ("search_result", "document")
                    for sb in tool_content
                )

                if not has_citable:
                    new_content.append(block)
                    continue

                has_changes = True
                new_sub_blocks: list[object] = []
                for sb in tool_content:
                    if not isinstance(sb, dict):
                        new_sub_blocks.append(sb)
                        continue

                    sb_type = sb.get("type")
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
                        new_sub_blocks.append(sb)

                new_block = cast(
                    ToolResultBlockParam,
                    {
                        **tool_block,
                        "content": new_sub_blocks,
                    },
                )
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
                            search_result_index=ref.citation_index,
                            title=ref.title,
                            source=ref.source,
                            cited_text=ref.cited_text[:200],
                        )
                    )
                elif ref.ref_type == "document":
                    citations.append(
                        CitationCharLocationParam(
                            type="char_location",
                            document_index=ref.citation_index,
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
    """Convert prompt-based citation markers into Anthropic-style text blocks.

    Each cited claim becomes a text block whose ``citations`` list contains the
    supporting references. Subsequent source blocks are reindexed when one input
    text block expands into multiple output blocks.
    """

    _PREFIX = "[citation:"

    def __init__(self, citable_index: dict[int, CitableRef]) -> None:
        self._citable_index = citable_index
        self._buf = ""
        self._index_map: dict[int, int] = {}
        self._next_output_index = 0
        self._active_input_index: int | None = None
        self._active_output_index: int | None = None
        self._text_block_open = False
        self._current_block_has_citations = False

    async def process(
        self,
        stream: AsyncIterator[MessageStreamEvent],
    ) -> AsyncIterator[MessageStreamEvent]:
        async for event in stream:
            if event.type == "content_block_start":
                output_index = self._next_output_index
                self._next_output_index += 1
                self._index_map[event.index] = output_index

                if event.content_block.type == "text":
                    self._active_input_index = event.index
                    self._active_output_index = output_index
                    self._text_block_open = True
                    self._current_block_has_citations = False
                    yield RawContentBlockStartEvent(
                        type="content_block_start",
                        index=output_index,
                        content_block=event.content_block.model_copy(
                            update={"text": "", "citations": None}
                        ),
                    )
                    if event.content_block.text:
                        for out in self._process_text(event.content_block.text):
                            yield out
                else:
                    yield event.model_copy(update={"index": output_index})

            elif event.type == "content_block_delta":
                if event.delta.type == "text_delta":
                    if event.index != self._active_input_index:
                        raise RuntimeError(
                            f"Text delta for inactive content block index {event.index}"
                        )
                    for out in self._process_text(event.delta.text):
                        yield out
                else:
                    output_index = (
                        self._current_text_index()
                        if event.index == self._active_input_index
                        else self._output_index(event.index)
                    )
                    yield event.model_copy(update={"index": output_index})

            elif event.type == "content_block_stop":
                if event.index == self._active_input_index:
                    for out in self._flush_buffer():
                        yield out
                    if self._text_block_open:
                        yield self._stop_text_block()
                    self._active_input_index = None
                    self._active_output_index = None
                    self._current_block_has_citations = False
                else:
                    yield event.model_copy(update={"index": self._output_index(event.index)})

            elif event.type == "message_stop":
                for out in self._finish_open_text_block():
                    yield out
                yield event

            else:
                yield event

        for out in self._finish_open_text_block():
            yield out

    def _process_text(self, text: str) -> list[MessageStreamEvent]:
        self._buf += text
        results: list[MessageStreamEvent] = []

        for segment in self._consume_buffer():
            if isinstance(segment, str):
                if not segment:
                    continue
                if self._current_block_has_citations:
                    results.append(self._stop_text_block())
                    results.append(self._start_text_block())
                results.append(
                    RawContentBlockDeltaEvent(
                        type="content_block_delta",
                        index=self._current_text_index(),
                        delta=TextDelta(type="text_delta", text=segment),
                    )
                )
            else:
                for ref_num in segment:
                    citation_event = self._build_citation_event(self._current_text_index(), ref_num)
                    if citation_event:
                        results.append(citation_event)
                        self._current_block_has_citations = True

        return results

    def _consume_buffer(self) -> list[str | list[int]]:
        """Return text and reference groups while retaining incomplete markers."""
        segments: list[str | list[int]] = []
        text_acc: list[str] = []

        def _flush_text() -> None:
            text = "".join(text_acc)
            text_acc.clear()
            if text:
                segments.append(text)

        while self._buf:
            bracket = self._buf.find("[")
            if bracket == -1:
                if self._buf.endswith(" "):
                    text_acc.append(self._buf[:-1])
                    self._buf = " "
                else:
                    text_acc.append(self._buf)
                    self._buf = ""
                break

            if bracket > 0:
                text_acc.append(self._buf[:bracket])
                self._buf = self._buf[bracket:]

            if len(self._buf) < len(self._PREFIX):
                if self._PREFIX.startswith(self._buf):
                    self._retain_trailing_space(text_acc)
                    break
                text_acc.append(self._buf[0])
                self._buf = self._buf[1:]
                continue

            if not self._buf.startswith(self._PREFIX):
                text_acc.append(self._buf[0])
                self._buf = self._buf[1:]
                continue

            close = self._buf.find("]", len(self._PREFIX))
            if close == -1:
                rest = self._buf[len(self._PREFIX) :]
                if all(c in "0123456789, " for c in rest):
                    self._retain_trailing_space(text_acc)
                    break
                text_acc.append(self._buf[0])
                self._buf = self._buf[1:]
                continue

            candidate = self._buf[: close + 1]
            match = _CITATION_PATTERN.fullmatch(candidate)
            if match:
                self._buf = self._buf[close + 1 :]
                if text_acc and text_acc[-1].endswith(" "):
                    text_acc[-1] = text_acc[-1][:-1]
                _flush_text()
                ref_nums = [
                    ref_num
                    for value in _NUM_PATTERN.findall(match.group(1))
                    if (ref_num := int(value)) in self._citable_index
                ]
                if ref_nums:
                    segments.append(ref_nums)
                continue

            text_acc.append(self._buf[0])
            self._buf = self._buf[1:]

        _flush_text()
        return segments

    def _retain_trailing_space(self, text_acc: list[str]) -> None:
        if not text_acc or not text_acc[-1].endswith(" "):
            return
        text_acc[-1] = text_acc[-1][:-1]
        self._buf = " " + self._buf

    def _flush_buffer(self) -> list[MessageStreamEvent]:
        if not self._buf:
            return []
        text = self._buf
        self._buf = ""
        return self._emit_unparsed_text(text)

    def _emit_unparsed_text(self, text: str) -> list[MessageStreamEvent]:
        results: list[MessageStreamEvent] = []
        if self._current_block_has_citations:
            results.append(self._stop_text_block())
            results.append(self._start_text_block())
        results.append(
            RawContentBlockDeltaEvent(
                type="content_block_delta",
                index=self._current_text_index(),
                delta=TextDelta(type="text_delta", text=text),
            )
        )
        return results

    def _finish_open_text_block(self) -> list[MessageStreamEvent]:
        if self._active_input_index is None:
            return []
        results = self._flush_buffer()
        if self._text_block_open:
            results.append(self._stop_text_block())
        self._active_input_index = None
        self._active_output_index = None
        self._current_block_has_citations = False
        return results

    def _start_text_block(self) -> RawContentBlockStartEvent:
        output_index = self._next_output_index
        self._next_output_index += 1
        self._active_output_index = output_index
        self._text_block_open = True
        self._current_block_has_citations = False
        return RawContentBlockStartEvent(
            type="content_block_start",
            index=output_index,
            content_block=TextBlock(type="text", text="", citations=None),
        )

    def _stop_text_block(self) -> RawContentBlockStopEvent:
        output_index = self._current_text_index()
        self._text_block_open = False
        return RawContentBlockStopEvent(
            type="content_block_stop",
            index=output_index,
        )

    def _current_text_index(self) -> int:
        if self._active_output_index is None or not self._text_block_open:
            raise RuntimeError("No active text content block")
        return self._active_output_index

    def _output_index(self, input_index: int) -> int:
        try:
            return self._index_map[input_index]
        except KeyError as error:
            raise RuntimeError(f"Event for unknown content block index {input_index}") from error

    def _build_citation_event(
        self, block_index: int, ref_num: int
    ) -> RawContentBlockDeltaEvent | None:
        ref = self._citable_index.get(ref_num)
        if ref is None:
            return None

        if ref.ref_type == "search_result":
            citation_obj = CitationsSearchResultLocation(
                type="search_result_location",
                search_result_index=ref.citation_index,
                start_block_index=0,
                end_block_index=0,
                title=ref.title,
                source=ref.source,
                cited_text=ref.cited_text[:200],
            )
        else:
            citation_obj = CitationCharLocation(
                type="char_location",
                document_index=ref.citation_index,
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
