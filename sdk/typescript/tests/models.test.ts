import { describe, it, expect } from 'vitest';
import {
  DocumentMetadataSchema,
  DocumentPermissionsSchema,
  DocumentSchema,
  ConnectorManifestSchema,
  SyncRequestSchema,
  EventType,
  serializeConnectorEvent,
  type ConnectorEventPayload,
} from '../src/models.js';

describe('DocumentMetadataSchema', () => {
  it('validates valid metadata', () => {
    const metadata = {
      title: 'Test Document',
      author: 'test@example.com',
      mime_type: 'text/plain',
      url: 'https://example.com/doc',
    };

    const result = DocumentMetadataSchema.safeParse(metadata);
    expect(result.success).toBe(true);
    if (result.success) {
      expect(result.data.title).toBe('Test Document');
      expect(result.data.author).toBe('test@example.com');
    }
  });

  it('allows empty metadata', () => {
    const result = DocumentMetadataSchema.safeParse({});
    expect(result.success).toBe(true);
  });

  it('validates datetime fields', () => {
    const metadata = {
      created_at: '2024-01-15T10:30:00Z',
      updated_at: '2024-01-15T11:00:00Z',
    };

    const result = DocumentMetadataSchema.safeParse(metadata);
    expect(result.success).toBe(true);
  });
});

describe('DocumentPermissionsSchema', () => {
  it('validates permissions with defaults', () => {
    const result = DocumentPermissionsSchema.safeParse({});
    expect(result.success).toBe(true);
    if (result.success) {
      expect(result.data.public).toBe(false);
      expect(result.data.users).toEqual([]);
      expect(result.data.groups).toEqual([]);
    }
  });

  it('validates full permissions', () => {
    const permissions = {
      public: true,
      users: ['user1@example.com', 'user2@example.com'],
      groups: ['engineering'],
    };

    const result = DocumentPermissionsSchema.safeParse(permissions);
    expect(result.success).toBe(true);
    if (result.success) {
      expect(result.data.public).toBe(true);
      expect(result.data.users).toHaveLength(2);
      expect(result.data.groups).toHaveLength(1);
    }
  });
});

describe('DocumentSchema', () => {
  it('validates a complete document', () => {
    const doc = {
      external_id: 'doc-123',
      title: 'My Document',
      content_id: 'content-456',
      metadata: { author: 'test@example.com' },
      permissions: { public: true },
    };

    const result = DocumentSchema.safeParse(doc);
    expect(result.success).toBe(true);
    if (result.success) {
      expect(result.data.external_id).toBe('doc-123');
      expect(result.data.title).toBe('My Document');
    }
  });

  it('requires external_id, title, and content_id', () => {
    const doc = { title: 'Missing ID' };
    const result = DocumentSchema.safeParse(doc);
    expect(result.success).toBe(false);
  });
});

describe('ConnectorManifestSchema', () => {
  it('validates a manifest', () => {
    const manifest = {
      name: 'my-connector',
      display_name: 'My Connector',
      version: '1.0.0',
      sync_modes: ['full', 'incremental'],
      connector_id: 'my-connector',
      connector_url: 'http://my-connector:8000',
      actions: [],
    };

    const result = ConnectorManifestSchema.safeParse(manifest);
    expect(result.success).toBe(true);
    if (result.success) {
      expect(result.data.name).toBe('my-connector');
      expect(result.data.display_name).toBe('My Connector');
      expect(result.data.sync_modes).toEqual(['full', 'incremental']);
    }
  });
});

describe('SyncRequestSchema', () => {
  it('validates a sync request', () => {
    const request = {
      sync_run_id: 'sync-123',
      source_id: 'source-456',
      sync_mode: 'full',
    };

    const result = SyncRequestSchema.safeParse(request);
    expect(result.success).toBe(true);
    if (result.success) {
      expect(result.data.sync_run_id).toBe('sync-123');
      expect(result.data.source_id).toBe('source-456');
    }
  });
});

describe('serializeConnectorEvent', () => {
  it('serializes document_created event', () => {
    const event: ConnectorEventPayload = {
      type: EventType.DOCUMENT_CREATED,
      sync_run_id: 'sync-123',
      source_id: 'source-456',
      document_id: 'doc-789',
      content_id: 'content-abc',
      metadata: { title: 'Test Doc' },
      permissions: { public: true, users: [], groups: [] },
    };

    const serialized = serializeConnectorEvent(event);

    expect(serialized.type).toBe('document_created');
    expect(serialized.sync_run_id).toBe('sync-123');
    expect(serialized.source_id).toBe('source-456');
    expect(serialized.document_id).toBe('doc-789');
    expect(serialized.content_id).toBe('content-abc');
    expect(serialized.metadata).toEqual({ title: 'Test Doc' });
    expect(serialized.permissions).toEqual({ public: true, users: [], groups: [] });
  });

  it('serializes document_deleted event without content fields', () => {
    const event: ConnectorEventPayload = {
      type: EventType.DOCUMENT_DELETED,
      sync_run_id: 'sync-123',
      source_id: 'source-456',
      document_id: 'doc-789',
    };

    const serialized = serializeConnectorEvent(event);

    expect(serialized.type).toBe('document_deleted');
    expect(serialized.sync_run_id).toBe('sync-123');
    expect(serialized.document_id).toBe('doc-789');
    expect(serialized.content_id).toBeUndefined();
    expect(serialized.metadata).toBeUndefined();
  });

  it('provides default permissions when not specified', () => {
    const event: ConnectorEventPayload = {
      type: EventType.DOCUMENT_CREATED,
      sync_run_id: 'sync-123',
      source_id: 'source-456',
      document_id: 'doc-789',
      content_id: 'content-abc',
    };

    const serialized = serializeConnectorEvent(event);

    expect(serialized.permissions).toEqual({ public: false, users: [], groups: [] });
    expect(serialized.metadata).toEqual({});
  });
});
