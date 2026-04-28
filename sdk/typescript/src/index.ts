export { Connector, type ServeOptions } from './connector.js';
export { SyncContext } from './context.js';
export { ContentStorage } from './storage.js';
export { SdkClient } from './client.js';
// McpAdapter (the runtime class) is not re-exported here to avoid requiring
// @modelcontextprotocol/sdk as a mandatory dependency. Import directly from
// './mcp-adapter.js' when needed. Type-only exports for the server config
// types are safe to surface here since they have no runtime cost.
export type {
  HttpMcpServer,
  McpAdapter,
  McpServer,
  StdioMcpServer,
} from './mcp-adapter.js';
export { createServer } from './server.js';

export {
  SyncMode,
  EventType,
  DocumentMetadataSchema,
  DocumentPermissionsSchema,
  DocumentSchema,
  ConnectorEventSchema,
  ActionDefinitionSchema,
  SearchOperatorSchema,
  McpResourceDefinitionSchema,
  McpPromptArgumentSchema,
  McpPromptDefinitionSchema,
  ConnectorManifestSchema,
  SyncRequestSchema,
  SyncResponseSchema,
  CancelRequestSchema,
  CancelResponseSchema,
  ActionRequestSchema,
  ActionResponseSchema,
  createSyncResponseStarted,
  createSyncResponseError,
  ActionResponse,
  serializeConnectorEvent,
  type DocumentMetadata,
  type DocumentPermissions,
  type Document,
  type ConnectorEvent,
  type ActionDefinition,
  type SearchOperator,
  type McpResourceDefinition,
  type McpPromptArgument,
  type McpPromptDefinition,
  type ConnectorManifest,
  type SyncRequest,
  type SyncResponse,
  type CancelRequest,
  type CancelResponse,
  type ActionRequest,
  type ConnectorEventPayload,
} from './models.js';

export {
  ConnectorError,
  SdkClientError,
  SyncCancelledError,
  ConfigurationError,
} from './errors.js';

export { getLogger } from './logger.js';
