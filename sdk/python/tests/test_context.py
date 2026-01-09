import json

import pytest

from omni_connector import Document, DocumentMetadata, DocumentPermissions, SyncContext


@pytest.mark.asyncio
async def test_emit_creates_document_created_event(sdk_client, mock_connector_manager):
    """Verify emit() creates a properly structured document_created event."""
    ctx = SyncContext(
        sdk_client=sdk_client,
        sync_run_id="sync-run-123",
        source_id="source-456",
    )

    doc = Document(
        external_id="doc-external-id",
        title="My Document Title",
        content_id="content-789",
        metadata=DocumentMetadata(
            author="author@example.com",
            url="https://example.com/doc",
        ),
        permissions=DocumentPermissions(
            public=False,
            users=["user1@example.com", "user2@example.com"],
        ),
        attributes={"department": "engineering"},
    )

    await ctx.emit(doc)

    # Verify the event payload
    call = mock_connector_manager.calls[0]
    payload = json.loads(call.request.content)

    assert payload["sync_run_id"] == "sync-run-123"
    assert payload["source_id"] == "source-456"

    event = payload["event"]
    assert event["type"] == "document_created"
    assert event["document_id"] == "doc-external-id"
    assert event["content_id"] == "content-789"
    assert event["metadata"]["author"] == "author@example.com"
    assert event["metadata"]["url"] == "https://example.com/doc"
    assert event["permissions"]["public"] is False
    assert event["permissions"]["users"] == ["user1@example.com", "user2@example.com"]
    assert event["attributes"]["department"] == "engineering"


@pytest.mark.asyncio
async def test_emit_updated_creates_document_updated_event(
    sdk_client, mock_connector_manager
):
    """Verify emit_updated() creates a document_updated event."""
    ctx = SyncContext(
        sdk_client=sdk_client,
        sync_run_id="sync-123",
        source_id="source-456",
    )

    doc = Document(
        external_id="doc-123",
        title="Updated Title",
        content_id="new-content-id",
    )

    await ctx.emit_updated(doc)

    payload = json.loads(mock_connector_manager.calls[0].request.content)
    assert payload["event"]["type"] == "document_updated"
    assert payload["event"]["document_id"] == "doc-123"
    assert payload["event"]["content_id"] == "new-content-id"


@pytest.mark.asyncio
async def test_emit_deleted_creates_document_deleted_event(
    sdk_client, mock_connector_manager
):
    """Verify emit_deleted() creates a minimal document_deleted event."""
    ctx = SyncContext(
        sdk_client=sdk_client,
        sync_run_id="sync-123",
        source_id="source-456",
    )

    await ctx.emit_deleted("doc-to-remove")

    payload = json.loads(mock_connector_manager.calls[0].request.content)
    event = payload["event"]

    assert event["type"] == "document_deleted"
    assert event["document_id"] == "doc-to-remove"
    assert event["sync_run_id"] == "sync-123"
    assert event["source_id"] == "source-456"
    # Deleted events should not have content/metadata
    assert "content_id" not in event
    assert "metadata" not in event


@pytest.mark.asyncio
async def test_complete_sends_correct_counts(sdk_client, mock_connector_manager):
    """Verify complete() sends accurate document counts."""
    ctx = SyncContext(
        sdk_client=sdk_client,
        sync_run_id="sync-123",
        source_id="source-456",
    )

    # Simulate scanning and emitting
    for i in range(5):
        await ctx.increment_scanned()

    doc = Document(external_id="doc-1", title="Doc", content_id="c1")
    await ctx.emit(doc)
    await ctx.emit(doc)
    await ctx.emit(doc)

    await ctx.complete(new_state={"cursor": "final"})

    # Find the complete call (last one)
    complete_calls = [
        c for c in mock_connector_manager.calls if "complete" in str(c.request.url)
    ]
    assert len(complete_calls) == 1

    payload = json.loads(complete_calls[0].request.content)
    assert payload["documents_scanned"] == 5
    assert payload["documents_updated"] == 3  # 3 emits
    assert payload["new_state"] == {"cursor": "final"}


@pytest.mark.asyncio
async def test_fail_sends_error_message(sdk_client, mock_connector_manager):
    """Verify fail() sends the error message."""
    ctx = SyncContext(
        sdk_client=sdk_client,
        sync_run_id="sync-123",
        source_id="source-456",
    )

    await ctx.fail("API rate limit exceeded")

    fail_calls = [
        c for c in mock_connector_manager.calls if "fail" in str(c.request.url)
    ]
    payload = json.loads(fail_calls[0].request.content)
    assert payload["error"] == "API rate limit exceeded"


@pytest.mark.asyncio
async def test_save_state_sends_heartbeat(sdk_client, mock_connector_manager):
    """Verify save_state() triggers a heartbeat."""
    ctx = SyncContext(
        sdk_client=sdk_client,
        sync_run_id="sync-123",
        source_id="source-456",
        state={"page": 1},
    )

    await ctx.save_state({"page": 2, "cursor": "abc"})

    # Should have made a heartbeat call
    heartbeat_calls = [
        c for c in mock_connector_manager.calls if "heartbeat" in str(c.request.url)
    ]
    assert len(heartbeat_calls) == 1
    assert "sync-123" in str(heartbeat_calls[0].request.url)

    # State should be updated locally
    assert ctx.state == {"page": 2, "cursor": "abc"}


@pytest.mark.asyncio
async def test_content_storage_save(sdk_client, mock_connector_manager):
    """Verify content_storage.save() stores content and returns ID."""
    ctx = SyncContext(
        sdk_client=sdk_client,
        sync_run_id="sync-123",
        source_id="source-456",
    )

    content_id = await ctx.content_storage.save(
        "<html><body>Hello World</body></html>",
        "text/html",
    )

    assert content_id == "test-content-id-123"

    # Verify the content was sent correctly
    content_calls = [
        c for c in mock_connector_manager.calls if "/sdk/content" in str(c.request.url)
    ]
    payload = json.loads(content_calls[0].request.content)
    assert payload["content"] == "<html><body>Hello World</body></html>"
    assert payload["content_type"] == "text/html"
    assert payload["sync_run_id"] == "sync-123"


@pytest.mark.asyncio
async def test_content_storage_save_bytes(sdk_client, mock_connector_manager):
    """Verify content_storage.save() handles bytes input."""
    ctx = SyncContext(
        sdk_client=sdk_client,
        sync_run_id="sync-123",
        source_id="source-456",
    )

    content_id = await ctx.content_storage.save(
        b"Binary content as bytes",
        "text/plain",
    )

    assert content_id == "test-content-id-123"

    content_calls = [
        c for c in mock_connector_manager.calls if "/sdk/content" in str(c.request.url)
    ]
    payload = json.loads(content_calls[0].request.content)
    assert payload["content"] == "Binary content as bytes"


def test_cancellation_flag():
    """Verify cancellation flag works correctly."""
    from omni_connector import SdkClient

    # Create a mock client (won't be used)
    class MockClient:
        base_url = "http://test"

    ctx = SyncContext(
        sdk_client=MockClient(),  # type: ignore
        sync_run_id="sync-123",
        source_id="source-456",
    )

    assert ctx.is_cancelled() is False

    ctx._set_cancelled()

    assert ctx.is_cancelled() is True


def test_context_exposes_properties():
    """Verify context properties are accessible."""

    class MockClient:
        base_url = "http://test"

    ctx = SyncContext(
        sdk_client=MockClient(),  # type: ignore
        sync_run_id="sync-run-id",
        source_id="source-id",
        state={"existing": "state"},
    )

    assert ctx.sync_run_id == "sync-run-id"
    assert ctx.source_id == "source-id"
    assert ctx.state == {"existing": "state"}
    assert ctx.documents_emitted == 0
    assert ctx.documents_scanned == 0
    assert ctx.content_storage is not None


@pytest.mark.asyncio
async def test_multiple_emits_increment_counter(sdk_client, mock_connector_manager):
    """Verify document counter increments correctly."""
    ctx = SyncContext(
        sdk_client=sdk_client,
        sync_run_id="sync-123",
        source_id="source-456",
    )

    doc = Document(external_id="doc", title="Doc", content_id="c")

    await ctx.emit(doc)
    assert ctx.documents_emitted == 1

    await ctx.emit(doc)
    assert ctx.documents_emitted == 2

    await ctx.emit_updated(doc)
    assert ctx.documents_emitted == 3

    # emit_deleted does NOT increment documents_emitted
    await ctx.emit_deleted("old-doc")
    assert ctx.documents_emitted == 3
