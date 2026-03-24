from .client import SdkClient
from .connector import Connector
from .context import SyncContext
from .exceptions import (
    ConfigurationError,
    ConnectorError,
    SdkClientError,
    SyncCancelledError,
)
from .models import (
    ActionDefinition,
    ActionParameter,
    ActionRequest,
    ActionResponse,
    CancelRequest,
    CancelResponse,
    ConnectorEvent,
    ConnectorManifest,
    Document,
    DocumentEvent,
    DocumentMetadata,
    DocumentPermissions,
    EventType,
    GroupMembershipSyncEvent,
    McpPromptArgument,
    McpPromptDefinition,
    McpResourceDefinition,
    SearchOperator,
    SyncMode,
    SyncRequest,
    SyncResponse,
)
from .storage import ContentStorage

__version__ = "0.1.0"

__all__ = [
    # Core classes
    "Connector",
    "SyncContext",
    "ContentStorage",
    "SdkClient",
    # Models
    "Document",
    "DocumentMetadata",
    "DocumentPermissions",
    "ConnectorEvent",
    "DocumentEvent",
    "GroupMembershipSyncEvent",
    "EventType",
    "ActionDefinition",
    "ActionParameter",
    "ActionRequest",
    "ActionResponse",
    "ConnectorManifest",
    "SearchOperator",
    "SyncMode",
    "SyncRequest",
    "SyncResponse",
    "CancelRequest",
    "CancelResponse",
    # MCP models
    "McpResourceDefinition",
    "McpPromptDefinition",
    "McpPromptArgument",
    # Exceptions
    "ConnectorError",
    "SdkClientError",
    "SyncCancelledError",
    "ConfigurationError",
]
