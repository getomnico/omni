"""Unit tests for ClickUp space-filtered sync."""

from collections.abc import AsyncIterator
from typing import Any

from clickup_connector import ClickUpConnector
from omni_connector import SyncMode


class FakeContentStorage:
    async def save(self, content: str, mime_type: str) -> str:
        assert mime_type == "text/plain"
        return f"content:{hash(content)}"


class FakeSyncContext:
    def __init__(self, sync_mode: SyncMode = SyncMode.FULL, *, is_resume: bool = False) -> None:
        self.content_storage = FakeContentStorage()
        self.documents_scanned = 0
        self.documents_emitted = 0
        self.emitted: list[Any] = []
        self.failed: str | None = None
        self.completed = False
        self.checkpoint: dict[str, Any] | None = None
        self.sync_mode = sync_mode
        self.is_resume = is_resume

    def is_cancelled(self) -> bool:
        return False

    async def fail(self, message: str) -> None:
        self.failed = message

    async def increment_scanned(self) -> None:
        self.documents_scanned += 1

    async def emit(self, document: Any) -> None:
        self.emitted.append(document)
        self.documents_emitted += 1

    async def emit_error(self, external_id: str, message: str) -> None:
        raise AssertionError(f"Unexpected sync error for {external_id}: {message}")

    async def emit_group_membership(
        self, group_email: str, member_emails: list[str], group_name: str | None = None
    ) -> None:
        pass

    async def save_checkpoint(self, checkpoint: dict[str, Any]) -> None:
        self.checkpoint = checkpoint

    async def complete(self, checkpoint: dict[str, Any] | None = None) -> None:
        self.completed = True
        self.checkpoint = checkpoint


class FakeClickUpClient:
    seen_date_updated_gt: list[int | None] = []

    def __init__(self, token: str, base_url: str | None = None) -> None:
        assert token == "pk_test"
        self.base_url = base_url

    async def get_workspaces(self) -> list[dict[str, Any]]:
        return [{"id": "team_1", "name": "Workspace", "members": []}]

    async def list_spaces(self, team_id: str) -> list[dict[str, Any]]:
        assert team_id == "team_1"
        return [
            {"id": "space_allowed", "name": "Engineering", "private": False},
            {"id": "space_blocked", "name": "Marketing", "private": False},
        ]

    async def list_folders(self, space_id: str) -> list[dict[str, Any]]:
        return []

    async def list_lists_in_folder(self, folder_id: str) -> list[dict[str, Any]]:
        return []

    async def list_folderless_lists(self, space_id: str) -> list[dict[str, Any]]:
        if space_id == "space_allowed":
            return [{"id": "list_allowed", "name": "Backlog"}]
        return [{"id": "list_blocked", "name": "Campaigns"}]

    async def list_tasks_page(
        self,
        team_id: str,
        page: int,
        *,
        include_closed: bool = True,
        subtasks: bool = True,
        date_updated_gt: int | None = None,
    ) -> list[dict[str, Any]]:
        assert team_id == "team_1"
        self.seen_date_updated_gt.append(date_updated_gt)
        if page > 0:
            return []
        return [
            _task("task_allowed", "Index me", "list_allowed"),
            _task("task_blocked", "Skip me", "list_blocked"),
        ]

    async def list_tasks(
        self,
        team_id: str,
        *,
        include_closed: bool = True,
        subtasks: bool = True,
        date_updated_gt: int | None = None,
    ) -> AsyncIterator[dict[str, Any]]:
        for task in await self.list_tasks_page(
            team_id,
            0,
            include_closed=include_closed,
            subtasks=subtasks,
            date_updated_gt=date_updated_gt,
        ):
            yield task

    async def get_task_comments(self, task_id: str) -> list[dict[str, Any]]:
        return []

    async def close(self) -> None:
        pass


def _task(task_id: str, name: str, list_id: str) -> dict[str, Any]:
    return {
        "id": task_id,
        "name": name,
        "description": "Description",
        "text_content": "Description",
        "status": {"status": "open"},
        "creator": {"username": "creator"},
        "assignees": [],
        "tags": [],
        "custom_fields": [],
        "list": {"id": list_id},
        "date_created": "1709280000000",
        "date_updated": "1709884800000",
    }


async def test_sync_indexes_only_selected_spaces(monkeypatch) -> None:
    FakeClickUpClient.seen_date_updated_gt = []
    monkeypatch.setattr("clickup_connector.connector.ClickUpClient", FakeClickUpClient)
    connector = ClickUpConnector()
    ctx = FakeSyncContext()

    await connector.sync(
        {"include_docs": False, "space_filters": ["space_allowed"]},
        {"token": "pk_test"},
        None,
        ctx,  # type: ignore[arg-type]
    )

    assert ctx.failed is None
    assert ctx.completed is True
    assert [doc.external_id for doc in ctx.emitted] == ["clickup:task:task_allowed"]


async def test_sync_empty_space_filters_indexes_everything(monkeypatch) -> None:
    FakeClickUpClient.seen_date_updated_gt = []
    monkeypatch.setattr("clickup_connector.connector.ClickUpClient", FakeClickUpClient)
    connector = ClickUpConnector()
    ctx = FakeSyncContext()

    await connector.sync(
        {"include_docs": False, "space_filters": []},
        {"token": "pk_test"},
        None,
        ctx,  # type: ignore[arg-type]
    )

    assert ctx.failed is None
    assert ctx.completed is True
    assert [doc.external_id for doc in ctx.emitted] == [
        "clickup:task:task_allowed",
        "clickup:task:task_blocked",
    ]


async def test_full_sync_ignores_previous_checkpoint(monkeypatch) -> None:
    FakeClickUpClient.seen_date_updated_gt = []
    monkeypatch.setattr("clickup_connector.connector.ClickUpClient", FakeClickUpClient)
    connector = ClickUpConnector()
    ctx = FakeSyncContext(sync_mode=SyncMode.FULL)

    await connector.sync(
        {"include_docs": False},
        {"token": "pk_test"},
        {
            "mode": "incremental",
            "workspaces": {
                "team_1": {
                    "last_task_updated_ts": 9999999999999,
                    "last_updated_ts": 9999999999999,
                }
            },
        },
        ctx,  # type: ignore[arg-type]
    )

    assert ctx.failed is None
    assert ctx.completed is True
    assert FakeClickUpClient.seen_date_updated_gt == [None]
    assert [doc.external_id for doc in ctx.emitted] == [
        "clickup:task:task_allowed",
        "clickup:task:task_blocked",
    ]
    assert ctx.checkpoint is not None
    assert ctx.checkpoint["mode"] == "full"


async def test_incremental_sync_uses_previous_checkpoint(monkeypatch) -> None:
    FakeClickUpClient.seen_date_updated_gt = []
    monkeypatch.setattr("clickup_connector.connector.ClickUpClient", FakeClickUpClient)
    connector = ClickUpConnector()
    ctx = FakeSyncContext(sync_mode=SyncMode.INCREMENTAL)

    await connector.sync(
        {"include_docs": False},
        {"token": "pk_test"},
        {
            "mode": "incremental",
            "workspaces": {
                "team_1": {
                    "last_task_updated_ts": 1700000000000,
                    "last_updated_ts": 1700000000000,
                }
            },
        },
        ctx,  # type: ignore[arg-type]
    )

    assert ctx.failed is None
    assert ctx.completed is True
    assert FakeClickUpClient.seen_date_updated_gt == [1700000000000]


async def test_resume_from_docs_phase_does_not_reprocess_tasks(monkeypatch) -> None:
    FakeClickUpClient.seen_date_updated_gt = []
    monkeypatch.setattr("clickup_connector.connector.ClickUpClient", FakeClickUpClient)
    connector = ClickUpConnector()
    ctx = FakeSyncContext(sync_mode=SyncMode.FULL, is_resume=True)

    await connector.sync(
        {"include_docs": False},
        {"token": "pk_test"},
        {
            "mode": "full",
            "workspaces": {
                "team_1": {
                    "last_task_updated_ts": 0,
                    "last_doc_updated_ts": 0,
                    "in_progress": {
                        "phase": "docs",
                        "latest_task_updated_ts": 1709884800000,
                        "latest_doc_updated_ts": 0,
                    },
                }
            },
        },
        ctx,  # type: ignore[arg-type]
    )

    assert ctx.failed is None
    assert ctx.completed is True
    assert FakeClickUpClient.seen_date_updated_gt == []
    assert ctx.emitted == []
    assert ctx.checkpoint is not None
    assert ctx.checkpoint["workspaces"]["team_1"]["last_task_updated_ts"] == 1709884800000
