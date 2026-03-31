"""Unit tests for synthetic citation logic (non-Anthropic providers)."""

from routers.chat import (
    _build_citable_index,
    _prepare_messages_for_non_citation_provider,
    _extract_synthetic_citations,
    _build_synthetic_citation_event,
)


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
    index = _build_citable_index(messages)
    assert len(index) == 2
    assert index[1].title == "Q3 Report"
    assert index[1].ref_type == "search_result"
    assert index[2].title == "Board Minutes"
    assert index[2].ref_type == "document"

    # 2. Transform messages for non-citation provider
    transformed = _prepare_messages_for_non_citation_provider(messages, index)
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

    # 3. Extract synthetic citations from model output
    text = "Revenue grew 15% [citation:1] and the board approved [citation:2] a new strategy."
    cleaned, citations = _extract_synthetic_citations(text, index)
    assert "[citation:" not in cleaned
    assert "Revenue grew 15%" in cleaned
    assert len(citations) == 2
    assert citations[0]["type"] == "search_result_location"
    assert citations[0]["title"] == "Q3 Report"
    assert citations[1]["type"] == "char_location"
    assert citations[1]["document_title"] == "Board Minutes"

    # Duplicate references should be deduplicated
    text_dup = "A [citation:1] and B [citation:1]."
    _, cits_dup = _extract_synthetic_citations(text_dup, index)
    assert len(cits_dup) == 1

    # Unknown references should be ignored
    text_unknown = "Something [citation:99]."
    _, cits_unknown = _extract_synthetic_citations(text_unknown, index)
    assert len(cits_unknown) == 0

    # 4. Build SSE events
    event = _build_synthetic_citation_event(0, citations[0])
    event_json = event.to_json()
    assert "citations_delta" in event_json
    assert "search_result_location" in event_json
