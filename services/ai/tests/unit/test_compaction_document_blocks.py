from unittest.mock import MagicMock

from anthropic.types import MessageParam

from services.compaction import ConversationCompactor


def _compactor() -> ConversationCompactor:
    return ConversationCompactor(llm_provider=MagicMock())


def _text_document(data: object) -> dict:
    return {
        "type": "document",
        "source": {"type": "text", "media_type": "text/plain", "data": data},
        "title": "Report",
    }


def test_estimate_tokens_counts_top_level_and_tool_result_documents() -> None:
    top_level = "top-level document content"
    tool_result = "tool-result document content"
    messages = [
        MessageParam(role="user", content=[_text_document(top_level)]),  # type: ignore[list-item]
        MessageParam(
            role="user",
            content=[
                {
                    "type": "tool_result",
                    "tool_use_id": "tool-1",
                    "content": [_text_document(tool_result)],
                }
            ],
        ),
    ]

    assert _compactor().estimate_tokens(messages) == (len(top_level) + len(tool_result)) // 4


def test_summary_format_includes_text_document_content() -> None:
    messages = [
        MessageParam(
            role="user",
            content=[
                {"type": "text", "text": "Mentioned document"},
                _text_document("The retained document context."),
            ],  # type: ignore[list-item]
        )
    ]

    formatted = _compactor()._format_messages_for_summary(messages)

    assert "Mentioned document" in formatted
    assert "The retained document context." in formatted


def test_summary_format_includes_tool_result_document_content() -> None:
    messages = [
        MessageParam(
            role="user",
            content=[
                {
                    "type": "tool_result",
                    "tool_use_id": "tool-1",
                    "content": [_text_document("Context returned by read_document.")],
                }
            ],
        )
    ]

    formatted = _compactor()._format_messages_for_summary(messages)

    assert "Context returned by read_document." in formatted


def test_malformed_text_document_data_is_ignored_safely() -> None:
    messages = [
        MessageParam(role="user", content=[_text_document(123)]),  # type: ignore[list-item]
    ]

    compactor = _compactor()
    assert compactor.estimate_tokens(messages) == 0
    assert "[Document block (text source)]" in compactor._format_messages_for_summary(messages)
