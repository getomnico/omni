"""Unit tests for citation logic."""

from services.citations import CitationProcessor


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

    # 4. Build SSE events
    event = CitationProcessor.build_event(0, citations[0])
    event_json = event.to_json()
    assert "citations_delta" in event_json
    assert "search_result_location" in event_json
