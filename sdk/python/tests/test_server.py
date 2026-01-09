"""Tests for the FastAPI server endpoints."""

from typing import Any
from unittest.mock import AsyncMock, patch

import pytest
from fastapi.testclient import TestClient

from omni_connector import (
    ActionDefinition,
    ActionParameter,
    ActionResponse,
    Connector,
    Document,
    DocumentMetadata,
    SyncContext,
)
from omni_connector.server import create_app


class MockConnector(Connector):
    """A test connector for testing the server."""

    def __init__(self):
        super().__init__()
        self.sync_called = False
        self.sync_args: tuple | None = None
        self.action_called = False
        self.action_args: tuple | None = None

    @property
    def name(self) -> str:
        return "test-connector"

    @property
    def version(self) -> str:
        return "1.2.3"

    @property
    def sync_modes(self) -> list[str]:
        return ["full", "incremental"]

    @property
    def actions(self) -> list[ActionDefinition]:
        return [
            ActionDefinition(
                name="test_action",
                description="A test action",
                parameters={
                    "param1": ActionParameter(type="string", required=True),
                    "param2": ActionParameter(type="number", required=False),
                },
            ),
        ]

    async def sync(
        self,
        source_config: dict[str, Any],
        credentials: dict[str, Any],
        state: dict[str, Any] | None,
        ctx: SyncContext,
    ) -> None:
        self.sync_called = True
        self.sync_args = (source_config, credentials, state)
        # Simulate emitting a document
        await ctx.emit(
            Document(
                external_id="test-doc",
                title="Test Document",
                content_id="content-123",
            )
        )
        await ctx.complete(new_state={"synced": True})

    async def execute_action(
        self,
        action: str,
        params: dict[str, Any],
        credentials: dict[str, Any],
    ) -> ActionResponse:
        self.action_called = True
        self.action_args = (action, params, credentials)

        if action == "test_action":
            return ActionResponse.success({"result": "action completed"})
        return ActionResponse.not_supported(action)


@pytest.fixture
def mock_connector():
    return MockConnector()


@pytest.fixture
def client(mock_connector):
    app = create_app(mock_connector)
    return TestClient(app)


class TestHealthEndpoint:
    def test_health_returns_status(self, client):
        response = client.get("/health")

        assert response.status_code == 200
        data = response.json()
        assert data["status"] == "healthy"
        assert data["service"] == "test-connector"


class TestManifestEndpoint:
    def test_manifest_returns_connector_info(self, client):
        response = client.get("/manifest")

        assert response.status_code == 200
        data = response.json()
        assert data["name"] == "test-connector"
        assert data["version"] == "1.2.3"
        assert data["sync_modes"] == ["full", "incremental"]

    def test_manifest_includes_actions(self, client):
        response = client.get("/manifest")

        data = response.json()
        assert len(data["actions"]) == 1

        action = data["actions"][0]
        assert action["name"] == "test_action"
        assert action["description"] == "A test action"
        assert "param1" in action["parameters"]
        assert action["parameters"]["param1"]["type"] == "string"
        assert action["parameters"]["param1"]["required"] is True


class TestCancelEndpoint:
    def test_cancel_not_found(self, client):
        """Cancel returns not_found when no matching sync is running."""
        response = client.post(
            "/cancel",
            json={"sync_run_id": "nonexistent-sync"},
        )

        assert response.status_code == 200
        data = response.json()
        assert data["status"] == "not_found"


class TestActionEndpoint:
    def test_action_executes_successfully(self, client, mock_connector):
        response = client.post(
            "/action",
            json={
                "action": "test_action",
                "params": {"param1": "value1", "param2": 42},
                "credentials": {"token": "secret"},
            },
        )

        assert response.status_code == 200
        data = response.json()
        assert data["status"] == "success"
        assert data["result"] == {"result": "action completed"}

        # Verify the connector received the correct args
        assert mock_connector.action_called is True
        action, params, credentials = mock_connector.action_args
        assert action == "test_action"
        assert params == {"param1": "value1", "param2": 42}
        assert credentials == {"token": "secret"}

    def test_action_not_supported(self, client):
        response = client.post(
            "/action",
            json={
                "action": "unknown_action",
                "params": {},
                "credentials": {},
            },
        )

        assert response.status_code == 200
        data = response.json()
        assert data["status"] == "error"
        assert "not supported" in data["error"]


class TestSyncEndpoint:
    @patch("omni_connector.server._fetch_source_data")
    def test_sync_returns_started(
        self, mock_fetch, client, mock_connector, monkeypatch
    ):
        """Verify sync endpoint returns immediately with 'started' status."""
        monkeypatch.setenv("CONNECTOR_MANAGER_URL", "http://localhost:9000")

        mock_fetch.return_value = (
            {"folder_id": "123"},  # source_config
            {"access_token": "token"},  # credentials
            {"cursor": "abc"},  # state
        )

        response = client.post(
            "/sync",
            json={
                "sync_run_id": "sync-123",
                "source_id": "source-456",
                "sync_mode": "full",
            },
        )

        assert response.status_code == 200
        data = response.json()
        assert data["status"] == "started"

    @patch("omni_connector.server._fetch_source_data")
    def test_sync_conflict_when_already_running(self, mock_fetch, monkeypatch):
        """Verify 409 when sync is already in progress for source."""
        import asyncio
        import threading

        monkeypatch.setenv("CONNECTOR_MANAGER_URL", "http://localhost:9000")

        # Create a connector that blocks until we signal it
        sync_started = threading.Event()
        sync_can_finish = threading.Event()

        class BlockingConnector(Connector):
            @property
            def name(self) -> str:
                return "blocking"

            @property
            def version(self) -> str:
                return "1.0.0"

            async def sync(self, source_config, credentials, state, ctx):
                sync_started.set()  # Signal that sync has started
                # Wait until test signals to finish
                while not sync_can_finish.is_set():
                    await asyncio.sleep(0.01)
                await ctx.complete()

        mock_fetch.return_value = ({}, {}, None)

        app = create_app(BlockingConnector())

        # Use a real async test client to properly handle background tasks
        with TestClient(app) as client:
            # First sync - this will start the sync in background
            response1 = client.post(
                "/sync",
                json={
                    "sync_run_id": "sync-1",
                    "source_id": "source-same",
                    "sync_mode": "full",
                },
            )
            assert response1.status_code == 200

            # Wait for the sync to actually start (enter the sync method)
            sync_started.wait(timeout=2.0)

            # Second sync for same source should conflict
            response2 = client.post(
                "/sync",
                json={
                    "sync_run_id": "sync-2",
                    "source_id": "source-same",
                    "sync_mode": "full",
                },
            )
            assert response2.status_code == 409
            assert "already in progress" in response2.json()["message"]

            # Let the first sync finish
            sync_can_finish.set()

    @patch("omni_connector.server._fetch_source_data")
    def test_sync_error_when_source_not_found(self, mock_fetch, client, monkeypatch):
        """Verify error when source doesn't exist."""
        from fastapi import HTTPException

        monkeypatch.setenv("CONNECTOR_MANAGER_URL", "http://localhost:9000")

        mock_fetch.side_effect = HTTPException(
            status_code=404,
            detail="Source not found: bad-source",
        )

        response = client.post(
            "/sync",
            json={
                "sync_run_id": "sync-123",
                "source_id": "bad-source",
                "sync_mode": "full",
            },
        )

        assert response.status_code == 404
        assert "not found" in response.json()["message"].lower()


class TestConnectorBaseClass:
    def test_connector_get_manifest(self, mock_connector):
        """Verify get_manifest() returns proper structure."""
        manifest = mock_connector.get_manifest()

        assert manifest.name == "test-connector"
        assert manifest.version == "1.2.3"
        assert manifest.sync_modes == ["full", "incremental"]
        assert len(manifest.actions) == 1

    def test_connector_cancel_adds_to_set(self, mock_connector):
        """Verify cancel() tracks the sync_run_id."""
        result = mock_connector.cancel("sync-to-cancel")

        assert result is True
        assert "sync-to-cancel" in mock_connector._cancelled_syncs

    def test_connector_default_actions_empty(self):
        """Verify default actions is empty list."""

        class MinimalConnector(Connector):
            @property
            def name(self) -> str:
                return "minimal"

            @property
            def version(self) -> str:
                return "0.0.1"

            async def sync(self, *args, **kwargs) -> None:
                pass

        connector = MinimalConnector()
        assert connector.actions == []
        assert connector.sync_modes == ["full"]
