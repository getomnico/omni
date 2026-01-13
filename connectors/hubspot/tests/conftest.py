"""Test fixtures for HubSpot connector tests."""

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
def mock_hubspot_contact():
    """Create a mock HubSpot contact object."""
    return {
        "id": "123",
        "properties": {
            "firstname": "John",
            "lastname": "Doe",
            "email": "john.doe@example.com",
            "phone": "+1234567890",
            "company": "Acme Corp",
            "jobtitle": "Engineer",
            "createdate": "2024-01-15T10:30:00Z",
            "hs_lastmodifieddate": "2024-06-01T14:00:00Z",
            "hubspot_owner_id": "owner-456",
        },
    }
