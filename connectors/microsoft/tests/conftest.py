"""Test fixtures for Microsoft connector tests."""

import pytest
import respx
from httpx import Response


@pytest.fixture
def mock_connector_manager():
    """Mock connector-manager SDK endpoints."""
    with respx.mock(
        base_url="http://localhost:9000", assert_all_called=False
    ) as respx_mock:
        respx_mock.post("/sdk/events").mock(
            return_value=Response(200, json={"status": "ok"})
        )

        respx_mock.post("/sdk/content").mock(
            return_value=Response(200, json={"content_id": "test-content-id-123"})
        )

        respx_mock.post(path__regex=r"/sdk/sync/.*/heartbeat").mock(
            return_value=Response(200, json={"status": "ok"})
        )

        respx_mock.post(path__regex=r"/sdk/sync/.*/scanned").mock(
            return_value=Response(200, json={"status": "ok"})
        )

        respx_mock.post(path__regex=r"/sdk/sync/.*/complete").mock(
            return_value=Response(200, json={"status": "ok"})
        )

        respx_mock.post(path__regex=r"/sdk/sync/.*/fail").mock(
            return_value=Response(200, json={"status": "ok"})
        )

        yield respx_mock


@pytest.fixture
def sdk_client(mock_connector_manager, monkeypatch):
    """Create SDK client with mocked endpoints."""
    monkeypatch.setenv("CONNECTOR_MANAGER_URL", "http://localhost:9000")
    from omni_connector import SdkClient

    return SdkClient.from_env()


@pytest.fixture
def mock_drive_item():
    """A OneDrive/SharePoint driveItem."""
    return {
        "id": "item-001",
        "name": "report.docx",
        "file": {
            "mimeType": "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
        },
        "size": 12345,
        "webUrl": "https://contoso-my.sharepoint.com/personal/user/Documents/report.docx",
        "createdDateTime": "2024-03-10T08:00:00Z",
        "lastModifiedDateTime": "2024-06-15T12:30:00Z",
        "parentReference": {
            "driveId": "drive-abc",
            "path": "/drive/root:/Documents",
        },
    }


@pytest.fixture
def mock_user():
    """A Microsoft Graph user object."""
    return {
        "id": "user-001",
        "displayName": "Alice Smith",
        "mail": "alice@contoso.com",
        "userPrincipalName": "alice@contoso.com",
    }
