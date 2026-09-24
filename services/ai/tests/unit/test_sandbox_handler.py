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
async def test_build_component_dispatches_typed_request(context: ToolContext):
    route = respx.post("http://sandbox.test/components/build").mock(
        return_value=httpx.Response(
            200,
            json={
                "path": "components/result.html",
                "size_bytes": 8192,
                "content_type": "text/html",
                "version": "build123",
            },
        )
    )

    handler = SandboxToolHandler("http://sandbox.test")
    result = await handler.execute(
        "build_component",
        {"source_path": "components/App.svelte", "output_path": "components/result.html"},
        context,
    )

    assert result.is_error is False
    request = json.loads(route.calls.last.request.content)
    assert request == {
        "source_path": "components/App.svelte",
        "output_path": "components/result.html",
        "chat_id": "chat-1",
    }
    assert json.loads(result.content[0]["text"])["content_type"] == "text/html"


@pytest.mark.asyncio
@respx.mock
async def test_build_component_rejects_traversal_before_dispatch(context: ToolContext):
    handler = SandboxToolHandler("http://sandbox.test")
    result = await handler.execute(
        "build_component",
        {"source_path": "../App.svelte", "output_path": "result.html"},
        context,
    )

    assert result.is_error is True
    assert ".." in result.content[0]["text"]
    assert not respx.calls


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
async def test_present_artifact_can_request_inline_html(context: ToolContext):
    respx.post("http://sandbox.test/files/stat").mock(
        return_value=httpx.Response(
            200,
            json={
                "path": "chart.html",
                "size_bytes": 4096,
                "content_type": "text/html",
                "exists": True,
                "version": "def456",
            },
        )
    )

    handler = SandboxToolHandler("http://sandbox.test")
    result = await handler.execute(
        "present_artifact",
        {
            "path": "chart.html",
            "title": "Interactive chart",
            "display_mode": "inline",
            "inline_height": 500,
        },
        context,
    )

    assert result.is_error is False
    info = json.loads(result.content[0]["text"])
    assert info["display_mode"] == "inline"
    assert info["inline_height"] == 500


@pytest.mark.asyncio
@respx.mock
async def test_present_artifact_rejects_non_html_inline_file(context: ToolContext):
    respx.post("http://sandbox.test/files/stat").mock(
        return_value=httpx.Response(
            200,
            json={
                "path": "report.pdf",
                "size_bytes": 4096,
                "content_type": "application/pdf",
                "exists": True,
                "version": "def456",
            },
        )
    )

    handler = SandboxToolHandler("http://sandbox.test")
    result = await handler.execute(
        "present_artifact",
        {"path": "report.pdf", "title": "Report", "display_mode": "inline"},
        context,
    )

    assert result.is_error is True
    assert "image or an HTML file" in result.content[0]["text"]


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
