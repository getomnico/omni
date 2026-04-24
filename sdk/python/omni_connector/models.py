from datetime import datetime
from enum import Enum
from typing import Annotated, Any, Literal, Self, Union

from fastapi.responses import JSONResponse
from pydantic import BaseModel, Discriminator, Field, Tag


class SyncMode(str, Enum):
    FULL = "full"
    INCREMENTAL = "incremental"
    REALTIME = "realtime"


class UserFilterMode(str, Enum):
    ALL = "all"
    WHITELIST = "whitelist"
    BLACKLIST = "blacklist"


class EventType(str, Enum):
    DOCUMENT_CREATED = "document_created"
    DOCUMENT_UPDATED = "document_updated"
    DOCUMENT_DELETED = "document_deleted"
    GROUP_MEMBERSHIP_SYNC = "group_membership_sync"


class DocumentMetadata(BaseModel):
    title: str | None = None
    author: str | None = None
    created_at: datetime | None = None
    updated_at: datetime | None = None
    content_type: str | None = None
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


class DocumentEvent(BaseModel):
    """Document create/update/delete event — mirrors Rust ConnectorEvent::Document* variants."""

    type: Literal["document_created", "document_updated", "document_deleted"]
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
            "type": self.type,
            "sync_run_id": self.sync_run_id,
            "source_id": self.source_id,
            "document_id": self.document_id,
        }
        if self.type == EventType.DOCUMENT_DELETED.value:
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


class GroupMembershipSyncEvent(BaseModel):
    """Group membership sync event — mirrors Rust ConnectorEvent::GroupMembershipSync."""

    type: Literal["group_membership_sync"] = "group_membership_sync"
    sync_run_id: str
    source_id: str
    group_email: str
    group_name: str | None = None
    member_emails: list[str] = Field(default_factory=list)

    def to_dict(self) -> dict[str, Any]:
        """Convert to dict format matching Rust tagged enum serialization."""
        result: dict[str, Any] = {
            "type": self.type,
            "sync_run_id": self.sync_run_id,
            "source_id": self.source_id,
            "group_email": self.group_email,
            "member_emails": self.member_emails,
        }
        if self.group_name:
            result["group_name"] = self.group_name
        return result


def _event_discriminator(v: Any) -> str:
    raw_type = v.get("type", "") if isinstance(v, dict) else getattr(v, "type", "")
    if raw_type == "group_membership_sync":
        return "group"
    return "document"


ConnectorEvent = Annotated[
    Union[
        Annotated[DocumentEvent, Tag("document")],
        Annotated[GroupMembershipSyncEvent, Tag("group")],
    ],
    Discriminator(_event_discriminator),
]


class ActionDefinition(BaseModel):
    name: str
    description: str
    input_schema: dict[str, Any] = Field(
        default_factory=lambda: {"type": "object", "properties": {}}
    )
    mode: str = "write"  # "read" or "write"


class SearchOperator(BaseModel):
    operator: str
    attribute_key: str
    value_type: str = "text"  # "person", "text", "datetime"


class McpResourceDefinition(BaseModel):
    uri_template: str
    name: str
    description: str | None = None
    mime_type: str | None = None


class McpPromptArgument(BaseModel):
    name: str
    description: str | None = None
    required: bool = False


class McpPromptDefinition(BaseModel):
    name: str
    description: str | None = None
    arguments: list[McpPromptArgument] = Field(default_factory=list)


class ConnectorManifest(BaseModel):
    name: str
    display_name: str
    version: str
    sync_modes: list[str]
    connector_id: str
    connector_url: str
    source_types: list[str] = Field(default_factory=list)
    description: str | None = None
    actions: list[ActionDefinition] = Field(default_factory=list)
    search_operators: list[SearchOperator] = Field(default_factory=list)
    extra_schema: dict | None = None
    attributes_schema: dict | None = None
    mcp_enabled: bool = False
    resources: list[McpResourceDefinition] = Field(default_factory=list)
    prompts: list[McpPromptDefinition] = Field(default_factory=list)


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
    def failure(cls, error: str) -> Self:
        return cls(status="error", error=error)

    @classmethod
    def not_supported(cls, action: str) -> Self:
        return cls(status="error", error=f"Action not supported: {action}")

    def to_response(self, status_code: int = 200) -> JSONResponse:
        """Convert this ActionResponse into a Starlette JSONResponse."""
        return JSONResponse(content=self.model_dump(), status_code=status_code)


class ResourceRequest(BaseModel):
    uri: str
    credentials: dict[str, Any] = Field(default_factory=dict)


class PromptRequest(BaseModel):
    name: str
    arguments: dict[str, Any] | None = None
    credentials: dict[str, Any] = Field(default_factory=dict)


class SdkSourceSyncData(BaseModel):
    """Typed envelope for the connector-manager /sdk/source/{id}/sync-config response.

    Matches the Rust SdkSourceSyncConfigResponse struct.
    """

    config: dict[str, Any]
    credentials: dict[str, Any]
    connector_state: dict[str, Any] | None = None
    source_type: str | None = None
    user_filter_mode: UserFilterMode = UserFilterMode.ALL
    user_whitelist: list[str] | None = None
    user_blacklist: list[str] | None = None
