import { z } from 'zod';

export const SyncMode = {
  FULL: 'full',
  INCREMENTAL: 'incremental',
} as const;
export type SyncMode = (typeof SyncMode)[keyof typeof SyncMode];

export const EventType = {
  DOCUMENT_CREATED: 'document_created',
  DOCUMENT_UPDATED: 'document_updated',
  DOCUMENT_DELETED: 'document_deleted',
} as const;
export type EventType = (typeof EventType)[keyof typeof EventType];

export const DocumentMetadataSchema = z.object({
  title: z.string().optional(),
  author: z.string().optional(),
  created_at: z.string().datetime().optional(),
  updated_at: z.string().datetime().optional(),
  mime_type: z.string().optional(),
  size: z.string().optional(),
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

export const ActionParameterSchema = z.object({
  type: z.string(),
  required: z.boolean().default(false),
  description: z.string().optional(),
});
export type ActionParameter = z.infer<typeof ActionParameterSchema>;

export const ActionDefinitionSchema = z.object({
  name: z.string(),
  description: z.string(),
  parameters: z.record(ActionParameterSchema).default({}),
  mode: z.enum(['read', 'write']).default('write'),
});
export type ActionDefinition = z.infer<typeof ActionDefinitionSchema>;

export const ConnectorManifestSchema = z.object({
  name: z.string(),
  version: z.string(),
  sync_modes: z.array(z.string()),
  actions: z.array(ActionDefinitionSchema).default([]),
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

export interface ConnectorEventPayload {
  type: EventType;
  sync_run_id: string;
  source_id: string;
  document_id: string;
  content_id?: string;
  metadata?: DocumentMetadata;
  permissions?: DocumentPermissions;
  attributes?: Record<string, unknown>;
}

export function serializeConnectorEvent(event: ConnectorEventPayload): Record<string, unknown> {
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
