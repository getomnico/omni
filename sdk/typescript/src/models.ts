import { z } from 'zod';

export const SyncMode = {
  FULL: 'full',
  INCREMENTAL: 'incremental',
  REALTIME: 'realtime',
} as const;
export type SyncMode = (typeof SyncMode)[keyof typeof SyncMode];

export const EventType = {
  DOCUMENT_CREATED: 'document_created',
  DOCUMENT_UPDATED: 'document_updated',
  DOCUMENT_DELETED: 'document_deleted',
  GROUP_MEMBERSHIP_SYNC: 'group_membership_sync',
} as const;
export type EventType = (typeof EventType)[keyof typeof EventType];

export const DocumentMetadataSchema = z.object({
  title: z.string().optional(),
  author: z.string().optional(),
  created_at: z.string().datetime().optional(),
  updated_at: z.string().datetime().optional(),
  mime_type: z.string().optional(),
  size: z.string().optional(),
  content_type: z.string().optional(),
  url: z.string().optional(),
  path: z.string().optional(),
  extra: z.record(z.unknown()).optional(),
});
export type DocumentMetadata = z.infer<typeof DocumentMetadataSchema>;

export const DocumentPermissionsSchema = z.object({
  public: z.boolean().default(false),
  users: z.array(z.string()).default([]),
  groups: z.array(z.string()).default([]),
});
export type DocumentPermissions = z.infer<typeof DocumentPermissionsSchema>;

export const DocumentSchema = z.object({
  external_id: z.string(),
  title: z.string(),
  content_id: z.string(),
  metadata: DocumentMetadataSchema.optional(),
  permissions: DocumentPermissionsSchema.optional(),
  attributes: z.record(z.unknown()).optional(),
});
export type Document = z.infer<typeof DocumentSchema>;

export const ConnectorEventSchema = z.object({
  type: z.enum(['document_created', 'document_updated', 'document_deleted']),
  sync_run_id: z.string(),
  source_id: z.string(),
  document_id: z.string(),
  content_id: z.string().optional(),
  metadata: DocumentMetadataSchema.optional(),
  permissions: DocumentPermissionsSchema.optional(),
  attributes: z.record(z.unknown()).optional(),
});
export type ConnectorEvent = z.infer<typeof ConnectorEventSchema>;

export const GroupMembershipEventSchema = z.object({
  type: z.literal('group_membership_sync'),
  sync_run_id: z.string(),
  source_id: z.string(),
  group_email: z.string(),
  group_name: z.string().optional(),
  member_emails: z.array(z.string()).default([]),
});
export type GroupMembershipEvent = z.infer<typeof GroupMembershipEventSchema>;

export const ActionDefinitionSchema = z.object({
  name: z.string(),
  description: z.string(),
  input_schema: z.record(z.any()).default({ type: 'object', properties: {} }),
  mode: z.enum(['read', 'write']).default('write'),
});
export type ActionDefinition = z.infer<typeof ActionDefinitionSchema>;

export const SearchOperatorSchema = z.object({
  operator: z.string(),
  attribute_key: z.string(),
  value_type: z.string().default('text'),  // "person", "text", "datetime"
});
export type SearchOperator = z.infer<typeof SearchOperatorSchema>;

export const McpResourceDefinitionSchema = z.object({
  uri_template: z.string(),
  name: z.string(),
  description: z.string().optional(),
  mime_type: z.string().optional(),
});
export type McpResourceDefinition = z.infer<typeof McpResourceDefinitionSchema>;

export const McpPromptArgumentSchema = z.object({
  name: z.string(),
  description: z.string().optional(),
  required: z.boolean().default(false),
});
export type McpPromptArgument = z.infer<typeof McpPromptArgumentSchema>;

export const McpPromptDefinitionSchema = z.object({
  name: z.string(),
  description: z.string().optional(),
  arguments: z.array(McpPromptArgumentSchema).default([]),
});
export type McpPromptDefinition = z.infer<typeof McpPromptDefinitionSchema>;

export const ConnectorManifestSchema = z.object({
  name: z.string(),
  display_name: z.string(),
  version: z.string(),
  sync_modes: z.array(z.string()),
  connector_id: z.string(),
  connector_url: z.string(),
  source_types: z.array(z.string()).default([]),
  description: z.string().optional(),
  actions: z.array(ActionDefinitionSchema).default([]),
  search_operators: z.array(SearchOperatorSchema).default([]),
  extra_schema: z.record(z.unknown()).optional(),
  attributes_schema: z.record(z.unknown()).optional(),
  mcp_enabled: z.boolean().default(false),
  resources: z.array(McpResourceDefinitionSchema).default([]),
  prompts: z.array(McpPromptDefinitionSchema).default([]),
});
export type ConnectorManifest = z.infer<typeof ConnectorManifestSchema>;

export const SyncRequestSchema = z.object({
  sync_run_id: z.string(),
  source_id: z.string(),
  sync_mode: z.string(),
});
export type SyncRequest = z.infer<typeof SyncRequestSchema>;

export const SyncResponseSchema = z.object({
  status: z.string(),
  message: z.string().optional(),
});
export type SyncResponse = z.infer<typeof SyncResponseSchema>;

export const CancelRequestSchema = z.object({
  sync_run_id: z.string(),
});
export type CancelRequest = z.infer<typeof CancelRequestSchema>;

export const CancelResponseSchema = z.object({
  status: z.string(),
});
export type CancelResponse = z.infer<typeof CancelResponseSchema>;

export const ActionRequestSchema = z.object({
  action: z.string(),
  params: z.record(z.unknown()),
  credentials: z.record(z.unknown()),
});
export type ActionRequest = z.infer<typeof ActionRequestSchema>;

export const ActionResponseSchema = z.object({
  status: z.string(),
  result: z.record(z.unknown()).optional(),
  error: z.string().optional(),
});
export type ActionResponse = z.infer<typeof ActionResponseSchema>;

export function createSyncResponseStarted(): SyncResponse {
  return { status: 'started' };
}

export function createSyncResponseError(message: string): SyncResponse {
  return { status: 'error', message };
}

export function createActionResponseSuccess(
  result: Record<string, unknown>
): ActionResponse {
  return { status: 'success', result };
}

export function createActionResponseFailure(error: string): ActionResponse {
  return { status: 'error', error };
}

export function createActionResponseNotSupported(action: string): ActionResponse {
  return { status: 'error', error: `Action not supported: ${action}` };
}

export class ActionResult {
  json?: ActionResponse;
  binary?: { data: Buffer; contentType: string };

  constructor(options: {
    json?: ActionResponse;
    binary?: { data: Buffer; contentType: string };
  }) {
    this.json = options.json;
    this.binary = options.binary;
  }

  static jsonResponse(response: ActionResponse): ActionResult {
    return new ActionResult({ json: response });
  }

  static binaryResponse(data: Buffer, contentType: string): ActionResult {
    return new ActionResult({ binary: { data, contentType } });
  }

  static notSupported(action: string): ActionResult {
    return ActionResult.jsonResponse(
      createActionResponseNotSupported(action)
    );
  }
}

export const ResourceRequestSchema = z.object({
  uri: z.string(),
  credentials: z.record(z.unknown()).default({}),
});
export type ResourceRequest = z.infer<typeof ResourceRequestSchema>;

export const PromptRequestSchema = z.object({
  name: z.string(),
  arguments: z.record(z.unknown()).optional(),
  credentials: z.record(z.unknown()).default({}),
});
export type PromptRequest = z.infer<typeof PromptRequestSchema>;

export interface DocumentEventPayload {
  type: typeof EventType.DOCUMENT_CREATED | typeof EventType.DOCUMENT_UPDATED | typeof EventType.DOCUMENT_DELETED;
  sync_run_id: string;
  source_id: string;
  document_id: string;
  content_id?: string;
  metadata?: DocumentMetadata;
  permissions?: DocumentPermissions;
  attributes?: Record<string, unknown>;
}

export interface GroupMembershipEventPayload {
  type: typeof EventType.GROUP_MEMBERSHIP_SYNC;
  sync_run_id: string;
  source_id: string;
  group_email: string;
  group_name?: string;
  member_emails: string[];
}

export type ConnectorEventPayload = DocumentEventPayload | GroupMembershipEventPayload;

export function serializeConnectorEvent(event: ConnectorEventPayload): Record<string, unknown> {
  if (event.type === EventType.GROUP_MEMBERSHIP_SYNC) {
    return {
      type: event.type,
      sync_run_id: event.sync_run_id,
      source_id: event.source_id,
      group_email: event.group_email,
      group_name: event.group_name,
      member_emails: event.member_emails,
    };
  }

  const base: Record<string, unknown> = {
    type: event.type,
    sync_run_id: event.sync_run_id,
    source_id: event.source_id,
    document_id: event.document_id,
  };

  if (event.type === EventType.DOCUMENT_DELETED) {
    return base;
  }

  base.content_id = event.content_id;
  base.metadata = event.metadata ?? {};
  base.permissions = event.permissions ?? { public: false, users: [], groups: [] };
  if (event.attributes) {
    base.attributes = event.attributes;
  }

  return base;
}
