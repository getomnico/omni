from anthropic.types import (
    MessageParam,
    TextBlockParam,
    ToolResultBlockParam,
    ToolUseBlockParam,
)

from evaluation.runners.chat_loop_runner import _extract_contexts, _extract_response


def _user_with_tool_result(tool_use_id: str, content, *, is_error: bool = False):
    return MessageParam(
        role="user",
        content=[
            ToolResultBlockParam(
                type="tool_result",
                tool_use_id=tool_use_id,
                content=content,
                is_error=is_error,
            )
        ],
    )


def test_extract_contexts_unwraps_search_result_blocks():
    """Production search_documents wraps highlights in search_result blocks
    alongside [Document ID: ...] / [URL: ...] metadata. Only the highlights
    are evidence."""
    final = [
        MessageParam(role="user", content="Q"),
        MessageParam(
            role="assistant",
            content=[
                ToolUseBlockParam(type="tool_use", id="t1", name="search_documents", input={}),
            ],
        ),
        _user_with_tool_result(
            "t1",
            [
                {
                    "type": "search_result",
                    "title": "Doc One",
                    "source": "http://x",
                    "content": [
                        {"type": "text", "text": "[Document ID: abc]"},
                        {"type": "text", "text": "[URL: http://x]"},
                        {"type": "text", "text": "Highlight one body."},
                        {"type": "text", "text": "Highlight two body."},
                    ],
                },
                {
                    "type": "search_result",
                    "title": "Doc Two",
                    "source": "http://y",
                    "content": [
                        {"type": "text", "text": "[Document ID: def]"},
                        {"type": "text", "text": "Other highlight."},
                    ],
                },
            ],
        ),
    ]
    assert _extract_contexts(final) == [
        "Highlight one body.",
        "Highlight two body.",
        "Other highlight.",
    ]


def test_extract_contexts_keeps_read_document_text():
    """read_document returns plain text blocks with the full chunk body."""
    final = [
        MessageParam(role="user", content="Q"),
        MessageParam(
            role="assistant",
            content=[
                ToolUseBlockParam(type="tool_use", id="t1", name="read_document", input={}),
            ],
        ),
        _user_with_tool_result(
            "t1",
            [
                {"type": "text", "text": "Full document body that the agent answered from."},
            ],
        ),
    ]
    assert _extract_contexts(final) == [
        "Full document body that the agent answered from."
    ]


def test_extract_contexts_skips_read_document_status_banners():
    """Sandbox-save/error banners from read_document are not evidence."""
    final = [
        MessageParam(role="user", content="Q"),
        _user_with_tool_result(
            "t1",
            [
                {
                    "type": "text",
                    "text": "Document saved to workspace: file.txt (42.0 KB). Use read_file or run_python to process it.",
                }
            ],
        ),
        _user_with_tool_result(
            "t2",
            [{"type": "text", "text": "Document not found: abc"}],
            is_error=True,
        ),
        _user_with_tool_result(
            "t3",
            [{"type": "text", "text": "read_document error: boom"}],
            is_error=True,
        ),
    ]
    assert _extract_contexts(final) == []


def test_extract_contexts_skips_errored_tool_results():
    """is_error tool_results are filtered even when the inner text looks valid."""
    final = [
        MessageParam(role="user", content="Q"),
        _user_with_tool_result(
            "t1",
            [{"type": "text", "text": "Looks like real content but the call failed."}],
            is_error=True,
        ),
    ]
    assert _extract_contexts(final) == []


def test_extract_contexts_skips_metadata_only_blocks():
    final = [
        MessageParam(role="user", content="Q"),
        _user_with_tool_result(
            "t1",
            [
                {"type": "text", "text": "[Document ID: abc]"},
                {"type": "text", "text": "  "},
                {"type": "text", "text": "Real chunk text."},
            ],
        ),
    ]
    assert _extract_contexts(final) == ["Real chunk text."]


def test_extract_response_uses_last_assistant_text():
    final = [
        MessageParam(role="user", content="Q"),
        MessageParam(
            role="assistant",
            content=[
                TextBlockParam(type="text", text="Final answer "),
                TextBlockParam(type="text", text="part two."),
            ],
        ),
    ]
    assert _extract_response(final) == "Final answer part two."


def test_extract_response_empty_when_no_assistant():
    assert _extract_response([MessageParam(role="user", content="Q")]) == ""
