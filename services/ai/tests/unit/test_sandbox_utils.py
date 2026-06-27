import httpx
import pytest
import respx

from tools.sandbox import write_binary_to_sandbox, write_text_to_sandbox


@pytest.mark.asyncio
@respx.mock
async def test_write_text_to_sandbox_returns_tool_error_on_payload_too_large():
    respx.post("http://sandbox.test/files/write").mock(
        return_value=httpx.Response(413, json={"detail": "Payload Too Large"})
    )

    result = await write_text_to_sandbox(
        "http://sandbox.test",
        "x" * 1024,
        "large_schema.json",
        "chat-1",
    )

    assert result.is_error is True
    assert "payload is too large" in result.content[0]["text"]
    assert "avoid recursively resolving large schemas" in result.content[0]["text"]


@pytest.mark.asyncio
@respx.mock
async def test_write_binary_to_sandbox_returns_tool_error_on_payload_too_large():
    respx.post("http://sandbox.test/files/write_binary").mock(
        return_value=httpx.Response(413, json={"detail": "Payload Too Large"})
    )

    result = await write_binary_to_sandbox(
        "http://sandbox.test",
        b"x" * 1024,
        "large.bin",
        "chat-1",
    )

    assert result.is_error is True
    assert "payload is too large" in result.content[0]["text"]


@pytest.mark.asyncio
@respx.mock
async def test_write_text_to_sandbox_returns_tool_error_on_other_write_failure():
    respx.post("http://sandbox.test/files/write").mock(
        return_value=httpx.Response(500, json={"detail": "disk full"})
    )

    result = await write_text_to_sandbox(
        "http://sandbox.test",
        "hello",
        "result.txt",
        "chat-1",
    )

    assert result.is_error is True
    assert (
        result.content[0]["text"]
        == "Could not save result.txt to the sandbox: disk full"
    )
