"""Unit tests for citation logic."""

from anthropic.types import (
    RawContentBlockDeltaEvent,
    RawContentBlockStopEvent,
    TextDelta,
)

from services.citations import CitationProcessor, CitationStreamProcessor, CitableRef


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


def test_citation_stream_processor():
    """CitationStreamProcessor strips markers from text deltas and emits citation events inline."""
    citable_index = {
        1: CitableRef(
            index=1,
            title="Doc A",
            source="http://a.com",
            cited_text="content a",
            ref_type="search_result",
        ),
        2: CitableRef(
            index=2,
            title="Doc B",
            source="http://b.com",
            cited_text="content b",
            ref_type="document",
        ),
    }
    proc = CitationStreamProcessor(citable_index)

    def make_text_delta(index: int, text: str) -> RawContentBlockDeltaEvent:
        return RawContentBlockDeltaEvent(
            type="content_block_delta",
            index=index,
            delta=TextDelta(type="text_delta", text=text),
        )

    def make_block_stop(index: int) -> RawContentBlockStopEvent:
        return RawContentBlockStopEvent(type="content_block_stop", index=index)

    # Simulate streaming: "Hello [citation:1] world"
    # Arrives as: "Hello ", "[citation:1]", " world"
    out1 = proc.process(make_text_delta(0, "Hello "))
    assert len(out1) == 1
    assert out1[0].delta.type == "text_delta"
    assert out1[0].delta.text == "Hello "

    out2 = proc.process(make_text_delta(0, "[citation:1]"))
    # The marker is swallowed; a citation event is emitted instead
    citation_events = [e for e in out2 if e.delta.type == "citations_delta"]
    text_events = [e for e in out2 if e.delta.type == "text_delta"]
    assert len(citation_events) == 1
    assert citation_events[0].delta.citation.type == "search_result_location"
    # No text should leak
    for te in text_events:
        assert "[citation:" not in te.delta.text

    out3 = proc.process(make_text_delta(0, " world"))
    assert any(e.delta.type == "text_delta" and " world" in e.delta.text for e in out3)

    # Test partial marker across chunks: "[cit" + "ation:2] done"
    proc2 = CitationStreamProcessor(citable_index)
    out_a = proc2.process(make_text_delta(0, "start [cit"))
    # "start " should be emitted, "[cit" buffered
    text_so_far = "".join(e.delta.text for e in out_a if e.delta.type == "text_delta")
    assert "start " in text_so_far
    assert "[cit" not in text_so_far

    out_b = proc2.process(make_text_delta(0, "ation:2] done"))
    citation_events_b = [e for e in out_b if e.delta.type == "citations_delta"]
    text_events_b = [e for e in out_b if e.delta.type == "text_delta"]
    assert len(citation_events_b) == 1
    assert citation_events_b[0].delta.citation.type == "char_location"
    all_text_b = "".join(e.delta.text for e in text_events_b)
    assert "[citation:" not in all_text_b
    assert "done" in all_text_b

    # Test whitespace around citation markers is consumed
    proc_ws = CitationStreamProcessor(citable_index)
    out_ws = proc_ws.process(make_text_delta(0, "version 10.0 [citation:1] is stable"))
    ws_text = "".join(e.delta.text for e in out_ws if e.delta.type == "text_delta")
    assert ws_text == "version 10.0 is stable"  # no double space

    # Citation marker before punctuation — no extra space left behind
    proc_punct = CitationStreamProcessor(citable_index)
    out_punct = proc_punct.process(
        make_text_delta(0, "The software version is 10.0 [citation: 1].")
    )
    punct_text = "".join(
        e.delta.text for e in out_punct if e.delta.type == "text_delta"
    )
    assert punct_text == "The software version is 10.0."

    # Test flush emits buffered non-citation text
    proc3 = CitationStreamProcessor(citable_index)
    proc3.process(make_text_delta(0, "text ["))
    flush_events = proc3.flush()
    flush_text = "".join(
        e.delta.text for e in flush_events if e.delta.type == "text_delta"
    )
    assert "[" in flush_text  # incomplete bracket flushed as-is
