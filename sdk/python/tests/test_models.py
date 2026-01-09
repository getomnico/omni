from datetime import datetime, timezone

from omni_connector import (
    ConnectorEvent,
    ConnectorManifest,
    Document,
    DocumentMetadata,
    DocumentPermissions,
    EventType,
    SyncRequest,
    SyncResponse,
)


def test_document_metadata_serialization():
    metadata = DocumentMetadata(
        title="Test Document",
        author="test@example.com",
        mime_type="text/plain",
        url="https://example.com/doc",
    )

    data = metadata.model_dump()
    assert data["title"] == "Test Document"
    assert data["author"] == "test@example.com"
    assert data["mime_type"] == "text/plain"
    assert data["url"] == "https://example.com/doc"


def test_document_metadata_with_datetime():
    now = datetime.now(timezone.utc)
    metadata = DocumentMetadata(
        title="Test",
        created_at=now,
        updated_at=now,
    )

    data = metadata.model_dump(mode="json")
    assert data["created_at"] is not None
    assert data["updated_at"] is not None


def test_document_permissions():
    permissions = DocumentPermissions(
        public=False,
        users=["user1@example.com", "user2@example.com"],
        groups=["engineering"],
    )

    data = permissions.model_dump()
    assert data["public"] is False
    assert data["users"] == ["user1@example.com", "user2@example.com"]
    assert data["groups"] == ["engineering"]


def test_document_permissions_defaults():
    permissions = DocumentPermissions()

    data = permissions.model_dump()
    assert data["public"] is False
    assert data["users"] == []
    assert data["groups"] == []


def test_document():
    doc = Document(
        external_id="doc-123",
        title="My Document",
        content_id="content-456",
        metadata=DocumentMetadata(author="test@example.com"),
        permissions=DocumentPermissions(public=True),
    )

    assert doc.external_id == "doc-123"
    assert doc.title == "My Document"
    assert doc.content_id == "content-456"
    assert doc.metadata.author == "test@example.com"
    assert doc.permissions.public is True


def test_connector_event_created_to_dict():
    event = ConnectorEvent(
        type=EventType.DOCUMENT_CREATED,
        sync_run_id="sync-123",
        source_id="source-456",
        document_id="doc-789",
        content_id="content-abc",
        metadata=DocumentMetadata(title="Test Doc"),
        permissions=DocumentPermissions(public=True),
    )

    data = event.to_dict()
    assert data["type"] == "document_created"
    assert data["sync_run_id"] == "sync-123"
    assert data["source_id"] == "source-456"
    assert data["document_id"] == "doc-789"
    assert data["content_id"] == "content-abc"
    assert data["metadata"]["title"] == "Test Doc"
    assert data["permissions"]["public"] is True


def test_connector_event_deleted_to_dict():
    event = ConnectorEvent(
        type=EventType.DOCUMENT_DELETED,
        sync_run_id="sync-123",
        source_id="source-456",
        document_id="doc-789",
    )

    data = event.to_dict()
    assert data["type"] == "document_deleted"
    assert data["sync_run_id"] == "sync-123"
    assert data["source_id"] == "source-456"
    assert data["document_id"] == "doc-789"
    assert "content_id" not in data
    assert "metadata" not in data


def test_connector_manifest():
    manifest = ConnectorManifest(
        name="my-connector",
        version="1.0.0",
        sync_modes=["full", "incremental"],
        actions=[],
    )

    data = manifest.model_dump()
    assert data["name"] == "my-connector"
    assert data["version"] == "1.0.0"
    assert data["sync_modes"] == ["full", "incremental"]


def test_sync_request():
    request = SyncRequest(
        sync_run_id="sync-123",
        source_id="source-456",
        sync_mode="full",
    )

    assert request.sync_run_id == "sync-123"
    assert request.source_id == "source-456"
    assert request.sync_mode == "full"


def test_sync_response_started():
    response = SyncResponse.started()
    assert response.status == "started"
    assert response.message is None


def test_sync_response_error():
    response = SyncResponse.error("Something went wrong")
    assert response.status == "error"
    assert response.message == "Something went wrong"
