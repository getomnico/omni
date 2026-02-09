"""Tests for connector sync: credential validation, item processing, cancellation."""

from unittest.mock import AsyncMock

import pytest

from ms_connector import MicrosoftConnector
from ms_connector.syncers.onedrive import OneDriveSyncer


async def test_connector_rejects_bad_credentials(sdk_client, mock_connector_manager):
    from omni_connector import SyncContext

    ctx = SyncContext(
        sdk_client=sdk_client,
        sync_run_id="sync-123",
        source_id="source-456",
    )

    connector = MicrosoftConnector()
    await connector.sync({}, {}, None, ctx)

    fail_calls = [
        call
        for call in mock_connector_manager.calls
        if "/fail" in str(call.request.url)
    ]
    assert len(fail_calls) == 1


async def test_onedrive_sync_with_mixed_items(
    mock_drive_item, mock_user, sdk_client, mock_connector_manager
):
    from omni_connector import SyncContext

    ctx = SyncContext(
        sdk_client=sdk_client,
        sync_run_id="sync-1",
        source_id="src-1",
    )

    folder_item = {
        "id": "folder-1",
        "name": "My Folder",
        "folder": {"childCount": 5},
        "parentReference": {"driveId": "drive-abc"},
    }
    deleted_item = {
        "id": "item-del",
        "deleted": {"state": "deleted"},
        "parentReference": {"driveId": "drive-abc"},
    }

    client = AsyncMock()
    client.get_delta = AsyncMock(
        return_value=([mock_drive_item, folder_item, deleted_item], "delta-token-new")
    )
    client.get_binary = AsyncMock(return_value=b"file content here")

    syncer = OneDriveSyncer()
    new_token = await syncer.sync_for_user(client, mock_user, ctx, None)

    assert new_token == "delta-token-new"
    assert ctx.documents_scanned == 3
    assert ctx.documents_emitted == 1


async def test_syncer_respects_cancellation(
    mock_drive_item, mock_user, sdk_client, mock_connector_manager
):
    from omni_connector import SyncContext

    ctx = SyncContext(
        sdk_client=sdk_client,
        sync_run_id="sync-1",
        source_id="src-1",
    )
    ctx._set_cancelled()

    client = AsyncMock()
    client.get_delta = AsyncMock(return_value=([mock_drive_item], "delta-token-new"))

    syncer = OneDriveSyncer()
    new_token = await syncer.sync_for_user(client, mock_user, ctx, "old-token")

    assert new_token == "old-token"
    assert ctx.documents_emitted == 0
