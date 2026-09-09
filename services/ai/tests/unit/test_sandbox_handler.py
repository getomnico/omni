import json

import httpx
import pytest
import respx

from tools.registry import ToolContext
from tools.sandbox_handler import SandboxToolHandler


@pytest.fixture
def context() -> ToolContext:
    return ToolContext(chat_id="chat-1", user_id="user-1")


@pytest.mark.asyncio
@respx.mock
async def test_edit_file_returns_success_message(context: ToolContext):
    respx.post("http://sandbox.test/files/edit").mock(
        return_value=httpx.Response(
            200, json={"content": "File edited successfully (1 replacement).", "path": "doc.md"}
        )
    )

    handler = SandboxToolHandler("http://sandbox.test")
    result = await handler.execute(
        "edit_file",
        {"path": "doc.md", "old_string": "old", "new_string": "new"},
        context,
    )

    assert result.is_error is False
    assert "File edited successfully" in result.content[0]["text"]


@pytest.mark.asyncio
@respx.mock
async def test_edit_file_surfaces_sandbox_error(context: ToolContext):
    respx.post("http://sandbox.test/files/edit").mock(
        return_value=httpx.Response(
            400,
            json={
                "detail": "old_string matches 3 locations. Either include more surrounding "
                "context to make it unique, or pass replace_all=true."
            },
        )
    )

    handler = SandboxToolHandler("http://sandbox.test")
    result = await handler.execute(
        "edit_file",
        {"path": "doc.md", "old_string": "old", "new_string": "new"},
        context,
    )

    assert result.is_error is True
    assert "3 locations" in result.content[0]["text"]


@pytest.mark.asyncio
@respx.mock
async def test_edit_file_sends_replace_all(context: ToolContext):
    route = respx.post("http://sandbox.test/files/edit").mock(
        return_value=httpx.Response(200, json={"content": "ok", "path": "doc.md"})
    )

    handler = SandboxToolHandler("http://sandbox.test")
    await handler.execute(
        "edit_file",
        {"path": "doc.md", "old_string": "old", "new_string": "new", "replace_all": True},
        context,
    )

    sent = json.loads(route.calls.last.request.content)
    assert sent["replace_all"] is True
    assert sent["chat_id"] == "chat-1"


@pytest.mark.asyncio
@respx.mock
async def test_present_artifact_pins_versioned_url(context: ToolContext):
    respx.post("http://sandbox.test/files/stat").mock(
        return_value=httpx.Response(
            200,
            json={
                "path": "report.docx",
                "size_bytes": 1234,
                "content_type": "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
                "exists": True,
                "version": "abc123",
            },
        )
    )

    handler = SandboxToolHandler("http://sandbox.test")
    result = await handler.execute(
        "present_artifact",
        {"path": "report.docx", "title": "Report"},
        context,
    )

    assert result.is_error is False
    info = json.loads(result.content[0]["text"])
    assert info["url"] == f"/api/chat/{context.chat_id}/artifacts/report.docx?v=abc123"
    assert info["version"] == "abc123"


@pytest.mark.asyncio
@respx.mock
async def test_present_artifact_without_version_keeps_plain_url(context: ToolContext):
    respx.post("http://sandbox.test/files/stat").mock(
        return_value=httpx.Response(
            200,
            json={
                "path": "chart.png",
                "size_bytes": 2048,
                "content_type": "image/png",
                "exists": True,
                "version": None,
            },
        )
    )

    handler = SandboxToolHandler("http://sandbox.test")
    result = await handler.execute(
        "present_artifact",
        {"path": "chart.png", "title": "Chart"},
        context,
    )

    assert result.is_error is False
    info = json.loads(result.content[0]["text"])
    assert info["url"] == f"/api/chat/{context.chat_id}/artifacts/chart.png"
    assert info["version"] is None
