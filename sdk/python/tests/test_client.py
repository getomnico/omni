import json

import pytest
from httpx import Response

from omni_connector import (
    ConnectorEvent,
    DocumentMetadata,
    DocumentPermissions,
    EventType,
)
from omni_connector.exceptions import SdkClientError


@pytest.mark.asyncio
async def test_emit_event_sends_correct_payload(sdk_client, mock_connector_manager):
    """Verify the exact JSON structure sent to connector-manager."""
    event = ConnectorEvent(
        type=EventType.DOCUMENT_CREATED,
        sync_run_id="sync-123",
        source_id="source-456",
        document_id="doc-789",
        content_id="content-abc",
        metadata=DocumentMetadata(title="Test Doc", author="test@example.com"),
        permissions=DocumentPermissions(public=True, users=["user1@example.com"]),
        attributes={"category": "test"},
    )

    await sdk_client.emit_event("sync-123", "source-456", event)

    # Verify the request was made
    assert len(mock_connector_manager.calls) == 1
    call = mock_connector_manager.calls[0]

    # Verify URL
    assert str(call.request.url) == "http://localhost:9000/sdk/events"

    # Verify payload structure matches what Rust expects
    payload = json.loads(call.request.content)
    assert payload["sync_run_id"] == "sync-123"
    assert payload["source_id"] == "source-456"

    # Verify event structure (tagged enum format)
    event_data = payload["event"]
    assert event_data["type"] == "document_created"
    assert event_data["sync_run_id"] == "sync-123"
    assert event_data["source_id"] == "source-456"
    assert event_data["document_id"] == "doc-789"
    assert event_data["content_id"] == "content-abc"
    assert event_data["metadata"]["title"] == "Test Doc"
    assert event_data["metadata"]["author"] == "test@example.com"
    assert event_data["permissions"]["public"] is True
    assert event_data["permissions"]["users"] == ["user1@example.com"]
    assert event_data["attributes"]["category"] == "test"


@pytest.mark.asyncio
async def test_emit_deleted_event_minimal_payload(sdk_client, mock_connector_manager):
    """Deleted events should not include content_id, metadata, or permissions."""
    event = ConnectorEvent(
        type=EventType.DOCUMENT_DELETED,
        sync_run_id="sync-123",
        source_id="source-456",
        document_id="doc-to-delete",
    )

    await sdk_client.emit_event("sync-123", "source-456", event)

    payload = json.loads(mock_connector_manager.calls[0].request.content)
    event_data = payload["event"]

    assert event_data["type"] == "document_deleted"
    assert event_data["document_id"] == "doc-to-delete"
    # Deleted events should NOT have these fields
    assert "content_id" not in event_data
    assert "metadata" not in event_data
    assert "permissions" not in event_data


@pytest.mark.asyncio
async def test_store_content_sends_correct_payload(sdk_client, mock_connector_manager):
    """Verify content storage request format."""
    content_id = await sdk_client.store_content(
        "sync-123",
        "This is the document content",
        "text/html",
    )

    assert content_id == "test-content-id-123"

    call = mock_connector_manager.calls[0]
    assert str(call.request.url) == "http://localhost:9000/sdk/content"

    payload = json.loads(call.request.content)
    assert payload["sync_run_id"] == "sync-123"
    assert payload["content"] == "This is the document content"
    assert payload["content_type"] == "text/html"


@pytest.mark.asyncio
async def test_complete_sends_correct_payload(sdk_client, mock_connector_manager):
    """Verify sync completion request includes all fields."""
    await sdk_client.complete(
        sync_run_id="sync-123",
        documents_scanned=150,
        documents_updated=42,
        new_state={"cursor": "abc123", "page": 5},
    )

    call = mock_connector_manager.calls[0]
    assert "/sdk/sync/sync-123/complete" in str(call.request.url)

    payload = json.loads(call.request.content)
    assert payload["documents_scanned"] == 150
    assert payload["documents_updated"] == 42
    assert payload["new_state"] == {"cursor": "abc123", "page": 5}


@pytest.mark.asyncio
async def test_complete_without_state(sdk_client, mock_connector_manager):
    """Verify completion works without new_state."""
    await sdk_client.complete(
        sync_run_id="sync-123",
        documents_scanned=10,
        documents_updated=5,
        new_state=None,
    )

    payload = json.loads(mock_connector_manager.calls[0].request.content)
    assert "new_state" not in payload


@pytest.mark.asyncio
async def test_fail_sends_error_message(sdk_client, mock_connector_manager):
    """Verify failure reports error correctly."""
    await sdk_client.fail("sync-123", "Connection timeout after 30s")

    call = mock_connector_manager.calls[0]
    assert "/sdk/sync/sync-123/fail" in str(call.request.url)

    payload = json.loads(call.request.content)
    assert payload["error"] == "Connection timeout after 30s"


@pytest.mark.asyncio
async def test_heartbeat_uses_correct_url(sdk_client, mock_connector_manager):
    """Verify heartbeat hits the right endpoint."""
    await sdk_client.heartbeat("sync-run-abc")

    call = mock_connector_manager.calls[0]
    assert (
        str(call.request.url) == "http://localhost:9000/sdk/sync/sync-run-abc/heartbeat"
    )


@pytest.mark.asyncio
async def test_increment_scanned_uses_correct_url(sdk_client, mock_connector_manager):
    """Verify scanned increment hits the right endpoint."""
    await sdk_client.increment_scanned("sync-run-xyz")

    call = mock_connector_manager.calls[0]
    assert (
        str(call.request.url) == "http://localhost:9000/sdk/sync/sync-run-xyz/scanned"
    )


@pytest.mark.asyncio
async def test_emit_event_raises_on_server_error(mock_connector_manager, monkeypatch):
    """Verify proper error handling on 500 response."""
    monkeypatch.setenv("CONNECTOR_MANAGER_URL", "http://localhost:9000")

    # Override the mock to return an error
    mock_connector_manager.post("/sdk/events").mock(
        return_value=Response(500, text="Internal Server Error")
    )

    from omni_connector import SdkClient

    client = SdkClient.from_env()

    event = ConnectorEvent(
        type=EventType.DOCUMENT_CREATED,
        sync_run_id="test",
        source_id="test",
        document_id="test",
        content_id="test",
    )

    with pytest.raises(SdkClientError) as exc_info:
        await client.emit_event("test", "test", event)

    assert "500" in str(exc_info.value)
    assert "Internal Server Error" in str(exc_info.value)


@pytest.mark.asyncio
async def test_store_content_raises_on_error(mock_connector_manager, monkeypatch):
    """Verify proper error handling when content storage fails."""
    monkeypatch.setenv("CONNECTOR_MANAGER_URL", "http://localhost:9000")

    mock_connector_manager.post("/sdk/content").mock(
        return_value=Response(413, text="Content too large")
    )

    from omni_connector import SdkClient

    client = SdkClient.from_env()

    with pytest.raises(SdkClientError) as exc_info:
        await client.store_content("sync-123", "x" * 1000000, "text/plain")

    assert "413" in str(exc_info.value)


@pytest.mark.asyncio
async def test_fetch_source_config_sends_correct_request(
    sdk_client, mock_connector_manager
):
    """Verify fetch_source_config calls the right endpoint and parses response."""
    mock_connector_manager.get("/sdk/source/source-123/sync-config").mock(
        return_value=Response(
            200,
            json={
                "config": {"folder_id": "abc"},
                "credentials": {"access_token": "token"},
                "connector_state": {"cursor": "xyz"},
            },
        )
    )

    result = await sdk_client.fetch_source_config("source-123")

    assert result["config"] == {"folder_id": "abc"}
    assert result["credentials"] == {"access_token": "token"}
    assert result["connector_state"] == {"cursor": "xyz"}

    call = mock_connector_manager.calls[-1]
    assert (
        str(call.request.url)
        == "http://localhost:9000/sdk/source/source-123/sync-config"
    )


@pytest.mark.asyncio
async def test_fetch_source_config_raises_on_404(mock_connector_manager, monkeypatch):
    """Verify proper error handling when source is not found."""
    monkeypatch.setenv("CONNECTOR_MANAGER_URL", "http://localhost:9000")

    mock_connector_manager.get(path__regex=r"/sdk/source/.*/sync-config").mock(
        return_value=Response(404, text="Source not found")
    )

    from omni_connector import SdkClient

    client = SdkClient.from_env()

    with pytest.raises(SdkClientError) as exc_info:
        await client.fetch_source_config("nonexistent-source")

    assert "404" in str(exc_info.value)


def test_client_requires_url(monkeypatch):
    """Verify client fails fast without CONNECTOR_MANAGER_URL."""
    monkeypatch.delenv("CONNECTOR_MANAGER_URL", raising=False)

    from omni_connector import SdkClient

    with pytest.raises(ValueError, match="CONNECTOR_MANAGER_URL"):
        SdkClient.from_env()


def test_client_strips_trailing_slash(monkeypatch):
    """Verify base URL normalization."""
    monkeypatch.setenv("CONNECTOR_MANAGER_URL", "http://localhost:9000/")

    from omni_connector import SdkClient

    client = SdkClient.from_env()

    assert client.base_url == "http://localhost:9000"
