"""Tests for HubSpotConnector class."""

import pytest

from hubspot_connector import HubSpotConnector


class TestHubSpotConnector:
    """Tests for the HubSpotConnector class."""

    @pytest.fixture
    def connector(self):
        """Create a HubSpotConnector instance."""
        return HubSpotConnector()

    @pytest.mark.asyncio
    async def test_sync_fails_without_access_token(
        self, connector, sdk_client, mock_connector_manager
    ):
        """Test sync fails when access_token is missing."""
        from omni_connector import SyncContext

        ctx = SyncContext(
            sdk_client=sdk_client,
            sync_run_id="sync-123",
            source_id="source-456",
        )

        await connector.sync({}, {}, None, ctx)

        fail_calls = [
            call
            for call in mock_connector_manager.calls
            if "/fail" in str(call.request.url)
        ]
        assert len(fail_calls) == 1

    @pytest.mark.asyncio
    async def test_cancel_marks_sync_cancelled(self, connector):
        """Test cancel() marks sync for cancellation."""
        result = connector.cancel("sync-123")
        assert result is True
        assert "sync-123" in connector._cancelled_syncs
