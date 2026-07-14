"""Unit tests for citation logic."""

import pytest
from anthropic.types import (
    InputJSONDelta,
    MessageStreamEvent,
    RawContentBlockDeltaEvent,
    RawContentBlockStartEvent,
    RawContentBlockStopEvent,
    RawMessageStopEvent,
    TextBlock,
    TextDelta,
    ToolUseBlock,
)

from services.citations import CitableRef, CitationProcessor, CitationStreamProcessor


def test_synthetic_citations_end_to_end():
    """Full pipeline: index → transform → extract → emit events."""
    messages = [
        {
            "role": "user",
            "content": [
                {
                    "type": "tool_result",
                    "tool_use_id": "call_1",
                    "content": [
                        {
                            "type": "search_result",
                            "title": "Q3 Report",
                            "source": "http://example.com/q3",
                            "content": [
                                {"type": "text", "text": "Revenue grew 15%."},
                                {"type": "text", "text": "Expenses dropped 3%."},
                            ],
                            "citations": {"enabled": True},
                        },
                        {
                            "type": "document",
                            "source": {
                                "type": "text",
                                "media_type": "text/plain",
                                "data": "The board approved the new strategy.",
                            },
                            "title": "Board Minutes",
                            "citations": {"enabled": True},
                        },
                    ],
                }
            ],
        }
    ]

    # 1. Build index
    index = CitationProcessor.build_citable_index(messages)
    assert len(index) == 2
    assert index[1].title == "Q3 Report"
    assert index[1].ref_type == "search_result"
    assert index[2].title == "Board Minutes"
    assert index[2].ref_type == "document"

    # 2. Transform messages for non-citation provider
    transformed = CitationProcessor.prepare_messages(messages, index)
    # Original should be untouched
    assert messages[0]["content"][0]["content"][0]["type"] == "search_result"
    # Transformed should have numbered text
    sub_blocks = transformed[0]["content"][0]["content"]
    assert sub_blocks[0]["type"] == "text"
    assert "[1]" in sub_blocks[0]["text"]
    assert "Q3 Report" in sub_blocks[0]["text"]
    assert sub_blocks[1]["type"] == "text"
    assert "[2]" in sub_blocks[1]["text"]
    assert "Board Minutes" in sub_blocks[1]["text"]

    # 2b. Citations on prior assistant text blocks should be stripped
    messages_with_assistant = messages + [
        {
            "role": "assistant",
            "content": [
                {
                    "type": "text",
                    "text": "Revenue grew 15%",
                    "citations": [
                        {
                            "type": "search_result_location",
                            "search_result_index": 0,
                            "start_block_index": 0,
                            "end_block_index": 0,
                            "title": "Q3 Report",
                            "source": "http://example.com/q3",
                            "cited_text": "Revenue grew 15%.",
                        }
                    ],
                }
            ],
        }
    ]
    index2 = CitationProcessor.build_citable_index(messages_with_assistant)
    transformed2 = CitationProcessor.prepare_messages(messages_with_assistant, index2)
    assistant_block = transformed2[1]["content"][0]
    assert assistant_block["type"] == "text"
    assert assistant_block["text"] == "Revenue grew 15%"
    assert "citations" not in assistant_block

    # 3. Extract synthetic citations from model output
    text = "Revenue grew 15% [citation:1] and the board approved [citation:2] a new strategy."
    cleaned, citations = CitationProcessor.extract_citations(text, index)
    assert "[citation:" not in cleaned
    assert "Revenue grew 15%" in cleaned
    assert len(citations) == 2
    assert citations[0]["type"] == "search_result_location"
    assert citations[0]["title"] == "Q3 Report"
    assert citations[1]["type"] == "char_location"
    assert citations[1]["document_title"] == "Board Minutes"

    # Duplicate references should be deduplicated
    text_dup = "A [citation:1] and B [citation:1]."
    _, cits_dup = CitationProcessor.extract_citations(text_dup, index)
    assert len(cits_dup) == 1

    # Unknown references should be ignored
    text_unknown = "Something [citation:99]."
    _, cits_unknown = CitationProcessor.extract_citations(text_unknown, index)
    assert len(cits_unknown) == 0

    # Comma-separated citations should be parsed
    text_multi = "Details here [citation:1, 2] and more."
    cleaned_multi, cits_multi = CitationProcessor.extract_citations(text_multi, index)
    assert len(cits_multi) == 2
    assert "[citation:" not in cleaned_multi

    # 4. Build SSE events
    event = CitationProcessor.build_event(0, citations[0])
    event_json = event.to_json()
    assert "citations_delta" in event_json
    assert "search_result_location" in event_json


def test_citable_refs_use_per_source_type_indices():
    messages = [
        {
            "role": "user",
            "content": [
                {
                    "type": "tool_result",
                    "tool_use_id": "call_1",
                    "content": [
                        {
                            "type": "search_result",
                            "title": "Search A",
                            "source": "http://example.com/a",
                            "content": [{"type": "text", "text": "search a"}],
                        },
                        {
                            "type": "document",
                            "source": {
                                "type": "text",
                                "media_type": "text/plain",
                                "data": "document a",
                            },
                            "title": "Document A",
                        },
                        {
                            "type": "search_result",
                            "title": "Search B",
                            "source": "http://example.com/b",
                            "content": [{"type": "text", "text": "search b"}],
                        },
                        {
                            "type": "document",
                            "source": {
                                "type": "text",
                                "media_type": "text/plain",
                                "data": "document b",
                            },
                            "title": "Document B",
                        },
                    ],
                }
            ],
        }
    ]

    index = CitationProcessor.build_citable_index(messages)
    assert [index[i].index for i in range(1, 5)] == [1, 2, 3, 4]
    assert [index[i].citation_index for i in range(1, 5)] == [0, 0, 1, 1]

    _, citations = CitationProcessor.extract_citations(
        "A [citation:1] B [citation:2] C [citation:3] D [citation:4]", index
    )
    assert citations[0]["search_result_index"] == 0
    assert citations[1]["document_index"] == 0
    assert citations[2]["search_result_index"] == 1
    assert citations[3]["document_index"] == 1

    stream_event = CitationStreamProcessor(index)._build_citation_event(0, 3)
    assert stream_event is not None
    assert stream_event.delta.citation.search_result_index == 1


def test_prepare_messages_preserves_non_citable_tool_result_content():
    messages = [
        {
            "role": "user",
            "content": [
                {
                    "type": "tool_result",
                    "tool_use_id": "call_1",
                    "content": [
                        {"type": "text", "text": "intro"},
                        "raw detail",
                        {
                            "type": "search_result",
                            "title": "Search A",
                            "source": "http://example.com/a",
                            "content": [{"type": "text", "text": "search a"}],
                        },
                        {"type": "text", "text": "outro"},
                    ],
                }
            ],
        }
    ]

    index = CitationProcessor.build_citable_index(messages)
    transformed = CitationProcessor.prepare_messages(messages, index)
    content = transformed[0]["content"][0]["content"]
    assert content[0] == {"type": "text", "text": "intro"}
    assert content[1] == "raw detail"
    assert content[2]["type"] == "text"
    assert content[2]["text"].startswith("[1] Search A")
    assert content[3] == {"type": "text", "text": "outro"}


async def _collect(stream) -> list:
    """Helper to collect an async iterator into a list."""
    return [event async for event in stream]


async def _async_iter(events):
    """Helper to turn a list into an async iterator."""
    for e in events:
        yield e


@pytest.mark.asyncio
async def test_citation_stream_processor():
    """Synthetic markers become cited text blocks with stable output indices."""
    citable_index = {
        1: CitableRef(
            index=1,
            citation_index=0,
            title="Doc A",
            source="http://a.com",
            cited_text="content a",
            ref_type="search_result",
        ),
        2: CitableRef(
            index=2,
            citation_index=0,
            title="Doc B",
            source="http://b.com",
            cited_text="content b",
            ref_type="document",
        ),
    }

    def start(index: int) -> RawContentBlockStartEvent:
        return RawContentBlockStartEvent(
            type="content_block_start",
            index=index,
            content_block=TextBlock(type="text", text="", citations=None),
        )

    def td(index: int, text: str) -> RawContentBlockDeltaEvent:
        return RawContentBlockDeltaEvent(
            type="content_block_delta",
            index=index,
            delta=TextDelta(type="text_delta", text=text),
        )

    def tool_start(index: int) -> RawContentBlockStartEvent:
        return RawContentBlockStartEvent(
            type="content_block_start",
            index=index,
            content_block=ToolUseBlock(
                type="tool_use",
                id="toolu_test",
                name="search_documents",
                input={},
            ),
        )

    def input_delta(index: int, partial_json: str) -> RawContentBlockDeltaEvent:
        return RawContentBlockDeltaEvent(
            type="content_block_delta",
            index=index,
            delta=InputJSONDelta(
                type="input_json_delta",
                partial_json=partial_json,
            ),
        )

    def stop(index: int) -> RawContentBlockStopEvent:
        return RawContentBlockStopEvent(type="content_block_stop", index=index)

    def assert_valid_block_lifecycle(events: list[MessageStreamEvent]) -> None:
        open_indices: set[int] = set()
        for event in events:
            if event.type == "content_block_start":
                assert event.index not in open_indices
                open_indices.add(event.index)
            elif event.type == "content_block_delta":
                assert event.index in open_indices
            elif event.type == "content_block_stop":
                assert event.index in open_indices
                open_indices.remove(event.index)
        assert not open_indices

    out = await _collect(
        CitationStreamProcessor(citable_index).process(
            _async_iter(
                [
                    start(0),
                    td(
                        0,
                        "First claim [citation:1]. Second claim [citation:2].",
                    ),
                    stop(0),
                    start(1),
                    td(1, "Following source block."),
                    stop(1),
                ]
            )
        )
    )

    assert_valid_block_lifecycle(out)

    block_texts: list[str] = []
    block_citation_types: list[list[str]] = []
    for event in out:
        if event.type == "content_block_start":
            assert event.index == len(block_texts)
            block_texts.append("")
            block_citation_types.append([])
        elif event.type == "content_block_delta":
            if event.delta.type == "text_delta":
                block_texts[event.index] += event.delta.text
            elif event.delta.type == "citations_delta":
                block_citation_types[event.index].append(event.delta.citation.type)

    assert block_texts == [
        "First claim",
        ". Second claim",
        ".",
        "Following source block.",
    ]
    assert block_citation_types == [
        ["search_result_location"],
        ["char_location"],
        [],
        [],
    ]
    assert [event.index for event in out if event.type == "content_block_stop"] == [
        0,
        1,
        2,
        3,
    ]

    # Tool blocks following a split text block are reindexed with their deltas.
    with_tool = await _collect(
        CitationStreamProcessor(citable_index).process(
            _async_iter(
                [
                    start(0),
                    td(0, "Claim [citation:1] tail"),
                    stop(0),
                    tool_start(1),
                    input_delta(1, '{"query":"test"}'),
                    stop(1),
                ]
            )
        )
    )
    assert_valid_block_lifecycle(with_tool)
    tool_events = [
        event
        for event in with_tool
        if (event.type == "content_block_start" and event.content_block.type == "tool_use")
        or (event.type == "content_block_delta" and event.delta.type == "input_json_delta")
    ]
    assert [event.index for event in tool_events] == [2, 2]
    assert with_tool[-1].type == "content_block_stop"
    assert with_tool[-1].index == 2

    # Separate adjacent markers support the same claim and stay in one block.
    adjacent = await _collect(
        CitationStreamProcessor(citable_index).process(
            _async_iter(
                [
                    start(0),
                    td(0, "Shared claim [citation:1] [citation:2]."),
                    stop(0),
                ]
            )
        )
    )
    adjacent_citations = [
        event
        for event in adjacent
        if event.type == "content_block_delta" and event.delta.type == "citations_delta"
    ]
    assert [event.index for event in adjacent_citations] == [0, 0]

    # Markers may span provider chunks.
    partial = await _collect(
        CitationStreamProcessor(citable_index).process(
            _async_iter(
                [
                    start(0),
                    td(0, "start "),
                    td(0, "[cit"),
                    td(0, "ation:2] done"),
                    stop(0),
                ]
            )
        )
    )
    assert "[citation:" not in "".join(
        event.delta.text
        for event in partial
        if event.type == "content_block_delta" and event.delta.type == "text_delta"
    )
    assert any(
        event.type == "content_block_delta"
        and event.delta.type == "citations_delta"
        and event.index == 0
        for event in partial
    )
    assert (
        next(
            event.delta.text
            for event in partial
            if event.type == "content_block_delta" and event.delta.type == "text_delta"
        )
        == "start"
    )

    # Incomplete markers are flushed and the synthesized block is closed before
    # message_stop when a provider omits content_block_stop.
    incomplete = await _collect(
        CitationStreamProcessor(citable_index).process(
            _async_iter(
                [
                    start(0),
                    td(0, "text ["),
                    RawMessageStopEvent(type="message_stop"),
                ]
            )
        )
    )
    assert incomplete[-1].type == "message_stop"
    assert incomplete[-2].type == "content_block_stop"
    assert (
        "".join(
            event.delta.text
            for event in incomplete
            if event.type == "content_block_delta" and event.delta.type == "text_delta"
        )
        == "text ["
    )
