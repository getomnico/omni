from types import SimpleNamespace
from unittest.mock import AsyncMock

import pytest

from tools.document_handler import (
    DIRECT_RETURN_THRESHOLD,
    PDF_EXTRACTION_FAILURE_REASON_PREFIX,
    PDF_EXTRACTION_FAILURE_TEXT,
    DocumentToolHandler,
    _is_pdf_extraction_failure,
)
from tools.registry import ToolContext, ToolResult


@pytest.mark.parametrize(
    ("content_type", "content", "expected"),
    [
        ("pdf", PDF_EXTRACTION_FAILURE_TEXT, True),
        (
            "application/pdf",
            f"{PDF_EXTRACTION_FAILURE_REASON_PREFIX}scanned document]",
            True,
        ),
        ("application/x-pdf", PDF_EXTRACTION_FAILURE_REASON_PREFIX + "]", False),
        ("pdf", "[Text extraction failed for this PDF. Near miss]", False),
        ("document", PDF_EXTRACTION_FAILURE_TEXT, False),
    ],
)
def test_pdf_extraction_failure_marker_is_strict(content_type, content, expected):
    assert _is_pdf_extraction_failure(content_type, content) is expected


@pytest.mark.asyncio
async def test_failed_mentioned_pdf_stages_binary_at_deterministic_path():
    storage = AsyncMock()
    storage.get_text.return_value = PDF_EXTRACTION_FAILURE_TEXT
    handler = DocumentToolHandler(
        content_storage=storage,
        sandbox_url="http://sandbox.test",
        connector_manager_url="http://connector.test",
    )
    handler._fetch_binary = AsyncMock(
        return_value=ToolResult(content=[{"type": "text", "text": "File staged"}])
    )
    doc = SimpleNamespace(
        id="doc-id",
        content_id="content-id",
        content_type="pdf",
        source_id="source-id",
    )
    context = ToolContext(chat_id="chat-id", user_id="user-id", user_email="user@example.com")

    result = await handler._read_text(
        doc,
        "report.pdf",
        None,
        None,
        context,
        deterministic_path="mention_doc-id_content-id_report.pdf",
    )

    assert not result.is_error
    handler._fetch_binary.assert_awaited_once_with(
        doc,
        "report.pdf",
        context,
        deterministic_path="mention_doc-id_content-id_report.pdf",
    )


@pytest.mark.asyncio
async def test_failed_pdf_without_staging_support_fails_closed():
    storage = AsyncMock()
    storage.get_text.return_value = PDF_EXTRACTION_FAILURE_TEXT
    handler = DocumentToolHandler(
        content_storage=storage,
        connector_manager_url="http://connector.test",
    )
    handler._fetch_binary = AsyncMock()
    doc = SimpleNamespace(
        content_id="content-id",
        content_type="pdf",
        source_id="source-id",
    )
    context = ToolContext(chat_id="chat-id", user_id="user-id", user_email="user@example.com")

    result = await handler._read_text(doc, "report.pdf", None, None, context)

    assert result.is_error
    assert "binary staging is unavailable" in result.content[0]["text"]
    handler._fetch_binary.assert_not_awaited()


@pytest.mark.asyncio
async def test_large_explicit_read_keeps_processing_guidance(monkeypatch):
    storage = AsyncMock()
    storage.get_text.return_value = "x" * (DIRECT_RETURN_THRESHOLD + 1)
    handler = DocumentToolHandler(content_storage=storage, sandbox_url="http://sandbox.test")
    doc = SimpleNamespace(
        id="doc-id",
        title="report",
        content_id="content-id",
        content_type="text/plain",
    )
    context = ToolContext(chat_id="chat-id", user_id="user-id", user_email="user@example.com")
    recorded_message = None

    async def fake_write_text(sandbox_url, text, file_name, chat_id, *, message=None):
        nonlocal recorded_message
        recorded_message = message
        return ToolResult(content=[{"type": "text", "text": message}])

    monkeypatch.setattr("tools.document_handler.write_text_to_sandbox", fake_write_text)

    result = await handler._read_text(doc, "report", None, None, context)

    assert not result.is_error
    assert recorded_message == (
        "Document saved to workspace: report.txt (31.3 KB). "
        "Use read_file or run_python to process it."
    )


@pytest.mark.asyncio
async def test_large_mentioned_read_uses_concise_workspace_notice(monkeypatch):
    storage = AsyncMock()
    storage.get_text.return_value = "x" * (DIRECT_RETURN_THRESHOLD + 1)
    handler = DocumentToolHandler(content_storage=storage, sandbox_url="http://sandbox.test")
    doc = SimpleNamespace(
        id="doc-id",
        title="report",
        content_id="content-id",
        content_type="text/plain",
    )
    context = ToolContext(chat_id="chat-id", user_id="user-id", user_email="user@example.com")
    recorded_message = None

    async def fake_write_text(sandbox_url, text, file_name, chat_id, *, message=None):
        nonlocal recorded_message
        recorded_message = message
        return ToolResult(content=[{"type": "text", "text": message}])

    monkeypatch.setattr("tools.document_handler.write_text_to_sandbox", fake_write_text)

    result = await handler._read_text(
        doc,
        "report",
        None,
        None,
        context,
        deterministic_path="mention_doc-id_content-id_report",
    )

    assert not result.is_error
    assert recorded_message == "File saved to workspace: mention_doc-id_content-id_report (31.3 KB)"
