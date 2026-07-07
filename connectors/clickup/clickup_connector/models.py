"""Typed representations of ClickUp objects and connector state."""

from __future__ import annotations

from dataclasses import dataclass, field
from enum import StrEnum
from typing import Any, Mapping, TypedDict


ROLE_GUEST = 4
CHECKPOINT_VERSION = 2


@dataclass(frozen=True)
class ClickUpMember:
    user_id: str
    username: str
    email: str | None
    role: int  # 1=Owner, 2=Admin, 3=Member, 4=Guest


@dataclass(frozen=True)
class ClickUpSpace:
    id: str
    name: str
    private: bool
    members: list[ClickUpMember] = field(default_factory=list)


@dataclass(frozen=True)
class ClickUpSourceConfig:
    api_url: str | None = None
    include_docs: bool = True
    space_filters: set[str] = field(default_factory=set)

    @classmethod
    def from_mapping(cls, raw: Mapping[str, object]) -> "ClickUpSourceConfig":
        api_url_value = raw.get("api_url")
        include_docs_value = raw.get("include_docs")
        space_filters_value = raw.get("space_filters")

        return cls(
            api_url=api_url_value if isinstance(api_url_value, str) else None,
            include_docs=include_docs_value if isinstance(include_docs_value, bool) else True,
            space_filters=_string_set(space_filters_value),
        )


class WorkspaceSyncPhase(StrEnum):
    TASKS = "tasks"
    DOCS = "docs"
    COMPLETE = "complete"


@dataclass(frozen=True)
class WorkspaceProgress:
    phase: WorkspaceSyncPhase
    task_page: int = 0
    task_offset: int = 0
    doc_cursor: str | None = None
    doc_offset: int = 0
    latest_task_updated_ts: int = 0
    latest_doc_updated_ts: int = 0

    @classmethod
    def from_mapping(cls, raw: Mapping[str, object]) -> "WorkspaceProgress | None":
        phase_value = raw.get("phase")
        if not isinstance(phase_value, str):
            return None
        try:
            phase = WorkspaceSyncPhase(phase_value)
        except ValueError:
            return None

        return cls(
            phase=phase,
            task_page=_int_value(raw.get("task_page")),
            task_offset=_int_value(raw.get("task_offset")),
            doc_cursor=_optional_str(raw.get("doc_cursor")),
            doc_offset=_int_value(raw.get("doc_offset")),
            latest_task_updated_ts=_int_value(raw.get("latest_task_updated_ts")),
            latest_doc_updated_ts=_int_value(raw.get("latest_doc_updated_ts")),
        )

    def to_json(self) -> dict[str, object]:
        data: dict[str, object] = {
            "phase": self.phase.value,
            "task_page": self.task_page,
            "task_offset": self.task_offset,
            "doc_offset": self.doc_offset,
            "latest_task_updated_ts": self.latest_task_updated_ts,
            "latest_doc_updated_ts": self.latest_doc_updated_ts,
        }
        if self.doc_cursor is not None:
            data["doc_cursor"] = self.doc_cursor
        return data


@dataclass(frozen=True)
class WorkspaceSyncState:
    last_task_updated_ts: int = 0
    last_doc_updated_ts: int = 0
    in_progress: WorkspaceProgress | None = None

    @classmethod
    def from_mapping(cls, raw: Mapping[str, object]) -> "WorkspaceSyncState":
        # Backward compatibility with the old checkpoint format.
        old_last_updated = _int_value(raw.get("last_updated_ts"))
        progress_value = raw.get("in_progress")
        progress = (
            WorkspaceProgress.from_mapping(progress_value)
            if isinstance(progress_value, Mapping)
            else None
        )
        return cls(
            last_task_updated_ts=_int_value(raw.get("last_task_updated_ts")) or old_last_updated,
            last_doc_updated_ts=_int_value(raw.get("last_doc_updated_ts")),
            in_progress=progress,
        )

    def with_progress(self, progress: WorkspaceProgress) -> "WorkspaceSyncState":
        return WorkspaceSyncState(
            last_task_updated_ts=self.last_task_updated_ts,
            last_doc_updated_ts=self.last_doc_updated_ts,
            in_progress=progress,
        )

    def completed(
        self,
        latest_task_updated_ts: int,
        latest_doc_updated_ts: int,
    ) -> "WorkspaceSyncState":
        return WorkspaceSyncState(
            last_task_updated_ts=max(self.last_task_updated_ts, latest_task_updated_ts),
            last_doc_updated_ts=max(self.last_doc_updated_ts, latest_doc_updated_ts),
            in_progress=None,
        )

    def to_json(self) -> dict[str, object]:
        data: dict[str, object] = {
            "last_task_updated_ts": self.last_task_updated_ts,
            "last_doc_updated_ts": self.last_doc_updated_ts,
            # Keep the legacy key populated for compatibility with older code/tests.
            "last_updated_ts": self.last_task_updated_ts,
        }
        if self.in_progress is not None:
            data["in_progress"] = self.in_progress.to_json()
        return data


@dataclass(frozen=True)
class ClickUpSyncCheckpoint:
    mode: str | None = None
    workspaces: dict[str, WorkspaceSyncState] = field(default_factory=dict)

    @classmethod
    def empty(cls, mode: str | None = None) -> "ClickUpSyncCheckpoint":
        return cls(mode=mode)

    @classmethod
    def from_mapping(cls, raw: Mapping[str, object] | None) -> "ClickUpSyncCheckpoint":
        if raw is None:
            return cls.empty()
        workspaces_value = raw.get("workspaces")
        workspaces: dict[str, WorkspaceSyncState] = {}
        if isinstance(workspaces_value, Mapping):
            for workspace_id, state_value in workspaces_value.items():
                if isinstance(workspace_id, str) and isinstance(state_value, Mapping):
                    workspaces[workspace_id] = WorkspaceSyncState.from_mapping(state_value)
        mode_value = raw.get("mode")
        return cls(
            mode=mode_value if isinstance(mode_value, str) else None,
            workspaces=workspaces,
        )

    def for_mode(self, mode: str, *, is_resume: bool) -> "ClickUpSyncCheckpoint":
        if is_resume:
            # Only trust a resume checkpoint for full sync if it was written by a
            # full sync run. Otherwise the manager may have fallen back to the
            # source's last completed incremental checkpoint.
            if mode == "full" and self.mode != "full":
                return ClickUpSyncCheckpoint.empty(mode=mode)
            return ClickUpSyncCheckpoint(mode=mode, workspaces=dict(self.workspaces))
        if mode == "incremental":
            return ClickUpSyncCheckpoint(mode=mode, workspaces=dict(self.workspaces))
        # Fresh full sync must ignore source-level checkpoints.
        return ClickUpSyncCheckpoint.empty(mode=mode)

    def with_workspace(
        self, workspace_id: str, state: WorkspaceSyncState
    ) -> "ClickUpSyncCheckpoint":
        workspaces = dict(self.workspaces)
        workspaces[workspace_id] = state
        return ClickUpSyncCheckpoint(mode=self.mode, workspaces=workspaces)

    def to_json(self) -> dict[str, object]:
        data: dict[str, object] = {
            "version": CHECKPOINT_VERSION,
            "workspaces": {
                workspace_id: state.to_json() for workspace_id, state in self.workspaces.items()
            },
        }
        if self.mode is not None:
            data["mode"] = self.mode
        return data


def parse_member(raw: Mapping[str, object]) -> ClickUpMember:
    """Parse a raw ClickUp API member dict into a ClickUpMember."""
    user_value = raw.get("user")
    user = user_value if isinstance(user_value, Mapping) else {}
    return ClickUpMember(
        user_id=str(user.get("id", "")),
        username=str(user.get("username", "")),
        email=_optional_str(user.get("email")),
        role=_int_value(raw.get("role")),
    )


def parse_space(raw: Mapping[str, object]) -> ClickUpSpace:
    """Parse a raw ClickUp API space dict into a ClickUpSpace."""
    members_value = raw.get("members")
    members = members_value if isinstance(members_value, list) else []
    return ClickUpSpace(
        id=str(raw["id"]),
        name=str(raw.get("name", "")),
        private=raw.get("private") is True,
        members=[parse_member(m) for m in members if isinstance(m, Mapping)],
    )


# ── ClickUp API response types ────────────────────────────────────
# TypedDicts for raw JSON responses from the ClickUp REST API. These are the
# shapes used by client.py and mappers.py. Not all API fields are modelled;
# only the ones the connector actually accesses are listed.


class ClickUpApiWorkspace(TypedDict, total=False):
    id: str
    name: str


class ClickUpApiSpace(TypedDict, total=False):
    id: str
    name: str
    private: bool
    members: list[dict[str, object]]


class ClickUpApiFolder(TypedDict, total=False):
    id: str
    name: str


class ClickUpApiList(TypedDict, total=False):
    id: str
    name: str
    orderindex: int
    space: dict[str, object]
    folder: dict[str, object]


class ClickUpApiUser(TypedDict, total=False):
    id: int
    username: str
    email: str


class ClickUpApiPriority(TypedDict, total=False):
    priority: str
    color: str


class ClickUpApiStatus(TypedDict, total=False):
    status: str
    color: str
    type: str


class ClickUpApiCreator(TypedDict, total=False):
    id: int
    username: str
    email: str


class ClickUpApiAssignee(TypedDict, total=False):
    id: int
    username: str
    email: str


class ClickUpApiTag(TypedDict, total=False):
    name: str


class ClickUpApiCustomField(TypedDict, total=False):
    id: str
    name: str
    value: object


class ClickUpApiTask(TypedDict, total=False):
    id: str
    name: str
    description: str
    text_content: str
    url: str
    status: ClickUpApiStatus
    priority: ClickUpApiPriority
    creator: ClickUpApiCreator
    assignees: list[ClickUpApiAssignee]
    tags: list[ClickUpApiTag]
    parent: str | None
    date_created: str
    date_updated: str
    due_date: str | None
    list: ClickUpApiList
    custom_fields: list[dict[str, object]]


class ClickUpApiComment(TypedDict, total=False):
    id: str
    comment_text: str
    user: ClickUpApiUser
    date: int


class ClickUpApiDoc(TypedDict, total=False):
    id: str
    name: str
    date_created: int
    date_updated: int


class ClickUpApiDocPage(TypedDict, total=False):
    id: str
    name: str
    content: str


def _int_value(value: object) -> int:
    if isinstance(value, bool):
        return 0
    if isinstance(value, int):
        return max(value, 0)
    if isinstance(value, str):
        try:
            return max(int(value), 0)
        except ValueError:
            return 0
    return 0


def _optional_str(value: object) -> str | None:
    return value if isinstance(value, str) and value else None


def _string_set(value: object) -> set[str]:
    if not isinstance(value, list):
        return set()
    return {item.strip() for item in value if isinstance(item, str) and item.strip()}
