from datetime import datetime
from enum import Enum
from typing import Any

from pydantic import BaseModel, Field


class SyncMode(str, Enum):
    FULL = "full"
    INCREMENTAL = "incremental"


class EventType(str, Enum):
    DOCUMENT_CREATED = "document_created"
    DOCUMENT_UPDATED = "document_updated"
    DOCUMENT_DELETED = "document_deleted"


class DocumentMetadata(BaseModel):
    title: str | None = None
    author: str | None = None
    created_at: datetime | None = None
    updated_at: datetime | None = None
    mime_type: str | None = None
    size: str | None = None
    url: str | None = None
    path: str | None = None
    extra: dict[str, Any] | None = None


class DocumentPermissions(BaseModel):
    public: bool = False
    users: list[str] = Field(default_factory=list)
    groups: list[str] = Field(default_factory=list)


class Document(BaseModel):
    external_id: str
    title: str
    content_id: str
    metadata: DocumentMetadata | None = None
    permissions: DocumentPermissions | None = None
    attributes: dict[str, Any] | None = None


class ConnectorEvent(BaseModel):
    type: EventType
    sync_run_id: str
    source_id: str
    document_id: str
    content_id: str | None = None
    metadata: DocumentMetadata | None = None
    permissions: DocumentPermissions | None = None
    attributes: dict[str, Any] | None = None

    def to_dict(self) -> dict[str, Any]:
        """Convert to dict format matching Rust tagged enum serialization."""
        base: dict[str, Any] = {
            "type": self.type.value,
            "sync_run_id": self.sync_run_id,
            "source_id": self.source_id,
            "document_id": self.document_id,
        }
        if self.type == EventType.DOCUMENT_DELETED:
            return base

        base["content_id"] = self.content_id
        if self.metadata:
            base["metadata"] = self.metadata.model_dump(mode="json", exclude_none=True)
        else:
            base["metadata"] = {}
        if self.permissions:
            base["permissions"] = self.permissions.model_dump()
        else:
            base["permissions"] = {"public": False, "users": [], "groups": []}
        if self.attributes:
            base["attributes"] = self.attributes
        return base


class ActionParameter(BaseModel):
    type: str
    required: bool = False
    description: str | None = None


class ActionDefinition(BaseModel):
    name: str
    description: str
    parameters: dict[str, ActionParameter] = Field(default_factory=dict)


class ConnectorManifest(BaseModel):
    name: str
    version: str
    sync_modes: list[str]
    actions: list[ActionDefinition] = Field(default_factory=list)


class SyncRequest(BaseModel):
    sync_run_id: str
    source_id: str
    sync_mode: str


class SyncResponse(BaseModel):
    status: str
    message: str | None = None

    @classmethod
    def started(cls) -> "SyncResponse":
        return cls(status="started")

    @classmethod
    def error(cls, message: str) -> "SyncResponse":
        return cls(status="error", message=message)


class CancelRequest(BaseModel):
    sync_run_id: str


class CancelResponse(BaseModel):
    status: str


class ActionRequest(BaseModel):
    action: str
    params: dict[str, Any]
    credentials: dict[str, Any]


class ActionResponse(BaseModel):
    status: str
    result: dict[str, Any] | None = None
    error: str | None = None

    @classmethod
    def success(cls, result: dict[str, Any]) -> "ActionResponse":
        return cls(status="success", result=result)

    @classmethod
    def failure(cls, error: str) -> "ActionResponse":
        return cls(status="error", error=error)

    @classmethod
    def not_supported(cls, action: str) -> "ActionResponse":
        return cls(status="error", error=f"Action not supported: {action}")
