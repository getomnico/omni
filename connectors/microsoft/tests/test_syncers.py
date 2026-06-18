"""Focused tests for Microsoft syncer checkpoint and delta edge cases."""

import copy
from typing import Any

import pytest

from ms_connector.graph_client import GraphAPIError
from ms_connector.syncers.base import BaseSyncer
from ms_connector.syncers.mail import MailSyncer
from ms_connector.syncers.onedrive import OneDriveSyncer
from ms_connector.syncers.teams import TeamsSyncer


class FakeContentStorage:
    async def save(self, content: str, mime_type: str) -> str:
        return "content-id"


class FakeContext:
    def __init__(self) -> None:
        self.deleted: list[str] = []
        self.saved_checkpoints: list[dict[str, Any]] = []
        self.scanned = 0
        self.content_storage = FakeContentStorage()

    def is_cancelled(self) -> bool:
        return False

    def should_index_user(self, email: str) -> bool:
        return True

    async def save_checkpoint(self, checkpoint: dict[str, Any]) -> None:
        self.saved_checkpoints.append(copy.deepcopy(checkpoint))

    async def emit_deleted(self, external_id: str) -> None:
        self.deleted.append(external_id)

    async def increment_scanned(self) -> None:
        self.scanned += 1


def _user(user_id: str = "u1") -> dict[str, str]:
    return {
        "id": user_id,
        "displayName": "Alice",
        "mail": "alice@contoso.com",
        "userPrincipalName": "alice@contoso.com",
    }


def _folder_item(item_id: str, drive_id: str = "drive-1") -> dict[str, Any]:
    return {
        "id": item_id,
        "name": item_id,
        "folder": {"childCount": 0},
        "parentReference": {"driveId": drive_id},
    }


async def test_onedrive_checkpoints_each_completed_delta_page() -> None:
    ctx = FakeContext()
    delta_tokens: dict[str, str] = {}

    class Client:
        async def get_delta_pages(self, *args: Any, **kwargs: Any):
            yield [_folder_item("folder-1")], "next-token", None
            yield [_folder_item("folder-2")], None, "delta-token"

    token = await OneDriveSyncer().sync_for_user(
        Client(),
        _user(),
        ctx,
        None,
        delta_tokens=delta_tokens,
        token_key="u1",
    )

    saved_tokens = [
        checkpoint["delta_tokens"]["u1"] for checkpoint in ctx.saved_checkpoints
    ]
    assert saved_tokens == ["next-token", "delta-token"]
    assert token == "delta-token"


async def test_onedrive_resyncs_once_when_delta_token_expires() -> None:
    ctx = FakeContext()

    class Client:
        def __init__(self) -> None:
            self.calls: list[str | None] = []

        async def get_delta_pages(self, *args: Any, **kwargs: Any):
            delta_token = kwargs.get("delta_token")
            self.calls.append(delta_token)
            if delta_token == "expired-token":
                raise GraphAPIError(
                    "Token expired",
                    status_code=410,
                    error_code="resyncRequired",
                )
            yield [_folder_item("folder-1")], None, "fresh-delta-token"

    client = Client()
    token = await OneDriveSyncer().sync_for_user(
        client,
        _user(),
        ctx,
        "expired-token",
        delta_tokens={},
        token_key="u1",
    )

    assert client.calls == ["expired-token", None]
    assert token == "fresh-delta-token"


async def test_mail_emits_deleted_documents_for_removed_delta_items() -> None:
    ctx = FakeContext()

    class Client:
        async def get_delta(self, *args: Any, **kwargs: Any):
            return ([{"id": "msg-1", "@removed": {"reason": "deleted"}}], "token-1")

    token = await MailSyncer()._sync_folder_for_user(
        Client(), _user(), ctx, "inbox", None
    )

    assert token == "token-1"
    assert ctx.deleted == ["mail:msg-1"]


async def test_mail_preserves_existing_tokens_when_folder_has_no_new_token() -> None:
    ctx = FakeContext()

    class Client:
        async def list_users(self) -> list[dict[str, str]]:
            return [_user()]

        async def get_delta(self, *args: Any, **kwargs: Any):
            return ([], None)

    checkpoint = {"delta_tokens": {"u1:inbox": "old-inbox", "u1:archive": "old-archive"}}
    result = await MailSyncer().sync(Client(), ctx, checkpoint)

    assert result["delta_tokens"] == checkpoint["delta_tokens"]


async def test_teams_preserves_existing_channel_and_chat_checkpoint() -> None:
    ctx = FakeContext()

    class Client:
        async def list_teams(self) -> list[dict[str, Any]]:
            return []

        async def list_users(self) -> list[dict[str, Any]]:
            return []

    checkpoint = {
        "delta_tokens": {"team:channel": "delta-token"},
        "last_sync_ts": {"team:channel": "2026-01-01T00:00:00+00:00"},
        "chat_last_sync_ts": {"chat-1": "2026-01-01T00:00:00+00:00"},
    }
    result = await TeamsSyncer().sync(Client(), ctx, checkpoint)

    assert result == checkpoint


async def test_base_syncer_fails_when_all_users_fail() -> None:
    ctx = FakeContext()

    class Client:
        async def list_users(self) -> list[dict[str, str]]:
            return [_user("u1"), _user("u2")]

    class FailingSyncer(BaseSyncer):
        @property
        def name(self) -> str:
            return "failing"

        async def sync_for_user(self, *args: Any, **kwargs: Any) -> str | None:
            raise RuntimeError("boom")

    with pytest.raises(RuntimeError, match="sync failed for all 2 users"):
        await FailingSyncer().sync(Client(), ctx, {})
