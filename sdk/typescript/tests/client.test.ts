import { describe, it, expect, beforeAll, afterAll, afterEach, vi } from 'vitest';
import { http, HttpResponse } from 'msw';
import { setupServer } from 'msw/node';
import { SdkClient } from '../src/client.js';
import { EventType, type ConnectorEventPayload } from '../src/models.js';

const BASE_URL = 'http://test-connector-manager:8080';

const server = setupServer();

beforeAll(() => {
  vi.stubEnv('CONNECTOR_MANAGER_URL', BASE_URL);
  server.listen({ onUnhandledRequest: 'error' });
});
afterEach(() => server.resetHandlers());
afterAll(() => {
  vi.unstubAllEnvs();
  server.close();
});

describe('SdkClient', () => {
  describe('emitEvent', () => {
    it('sends correct payload for document_created event', async () => {
      let capturedBody: unknown;

      server.use(
        http.post(`${BASE_URL}/sdk/events`, async ({ request }) => {
          capturedBody = await request.json();
          return HttpResponse.json({ success: true });
        })
      );

      const client = new SdkClient(BASE_URL);
      const event: ConnectorEventPayload = {
        type: EventType.DOCUMENT_CREATED,
        sync_run_id: 'sync-123',
        source_id: 'source-456',
        document_id: 'doc-789',
        content_id: 'content-abc',
        metadata: { title: 'Test Doc', author: 'test@example.com' },
        permissions: { public: true, users: [], groups: [] },
      };

      await client.emitEvent('sync-123', 'source-456', event);

      expect(capturedBody).toEqual({
        sync_run_id: 'sync-123',
        source_id: 'source-456',
        event: {
          type: 'document_created',
          sync_run_id: 'sync-123',
          source_id: 'source-456',
          document_id: 'doc-789',
          content_id: 'content-abc',
          metadata: { title: 'Test Doc', author: 'test@example.com' },
          permissions: { public: true, users: [], groups: [] },
        },
      });
    });

    it('sends correct payload for document_deleted event', async () => {
      let capturedBody: unknown;

      server.use(
        http.post(`${BASE_URL}/sdk/events`, async ({ request }) => {
          capturedBody = await request.json();
          return HttpResponse.json({ success: true });
        })
      );

      const client = new SdkClient(BASE_URL);
      const event: ConnectorEventPayload = {
        type: EventType.DOCUMENT_DELETED,
        sync_run_id: 'sync-123',
        source_id: 'source-456',
        document_id: 'doc-789',
      };

      await client.emitEvent('sync-123', 'source-456', event);

      expect(capturedBody).toEqual({
        sync_run_id: 'sync-123',
        source_id: 'source-456',
        event: {
          type: 'document_deleted',
          sync_run_id: 'sync-123',
          source_id: 'source-456',
          document_id: 'doc-789',
        },
      });
    });
  });

  describe('storeContent', () => {
    it('sends correct payload and returns content_id', async () => {
      let capturedBody: unknown;

      server.use(
        http.post(`${BASE_URL}/sdk/content`, async ({ request }) => {
          capturedBody = await request.json();
          return HttpResponse.json({ content_id: 'content-xyz-123' });
        })
      );

      const client = new SdkClient(BASE_URL);
      const contentId = await client.storeContent(
        'sync-123',
        'Hello, World!',
        'text/plain'
      );

      expect(capturedBody).toEqual({
        sync_run_id: 'sync-123',
        content: 'Hello, World!',
        content_type: 'text/plain',
      });
      expect(contentId).toBe('content-xyz-123');
    });
  });

  describe('heartbeat', () => {
    it('calls correct endpoint', async () => {
      let calledPath = '';

      server.use(
        http.post(`${BASE_URL}/sdk/sync/:id/heartbeat`, ({ params }) => {
          calledPath = `/sdk/sync/${params.id}/heartbeat`;
          return HttpResponse.json({ success: true });
        })
      );

      const client = new SdkClient(BASE_URL);
      await client.heartbeat('sync-run-abc');

      expect(calledPath).toBe('/sdk/sync/sync-run-abc/heartbeat');
    });
  });

  describe('incrementScanned', () => {
    it('calls correct endpoint', async () => {
      let calledPath = '';

      server.use(
        http.post(`${BASE_URL}/sdk/sync/:id/scanned`, ({ params }) => {
          calledPath = `/sdk/sync/${params.id}/scanned`;
          return HttpResponse.json({ success: true });
        })
      );

      const client = new SdkClient(BASE_URL);
      await client.incrementScanned('sync-run-abc');

      expect(calledPath).toBe('/sdk/sync/sync-run-abc/scanned');
    });
  });

  describe('complete', () => {
    it('sends correct payload with state', async () => {
      let capturedBody: unknown;

      server.use(
        http.post(`${BASE_URL}/sdk/sync/:id/complete`, async ({ request }) => {
          capturedBody = await request.json();
          return HttpResponse.json({ success: true });
        })
      );

      const client = new SdkClient(BASE_URL);
      await client.complete('sync-123', 100, 50, { cursor: 'abc123' });

      expect(capturedBody).toEqual({
        documents_scanned: 100,
        documents_updated: 50,
        new_state: { cursor: 'abc123' },
      });
    });

    it('sends payload without state when not provided', async () => {
      let capturedBody: unknown;

      server.use(
        http.post(`${BASE_URL}/sdk/sync/:id/complete`, async ({ request }) => {
          capturedBody = await request.json();
          return HttpResponse.json({ success: true });
        })
      );

      const client = new SdkClient(BASE_URL);
      await client.complete('sync-123', 100, 50);

      expect(capturedBody).toEqual({
        documents_scanned: 100,
        documents_updated: 50,
      });
      expect((capturedBody as Record<string, unknown>).new_state).toBeUndefined();
    });
  });

  describe('fail', () => {
    it('sends correct error payload', async () => {
      let capturedBody: unknown;

      server.use(
        http.post(`${BASE_URL}/sdk/sync/:id/fail`, async ({ request }) => {
          capturedBody = await request.json();
          return HttpResponse.json({ success: true });
        })
      );

      const client = new SdkClient(BASE_URL);
      await client.fail('sync-123', 'Something went wrong');

      expect(capturedBody).toEqual({
        error: 'Something went wrong',
      });
    });
  });

  describe('fetchSourceConfig', () => {
    it('sends correct request and parses response', async () => {
      const mockData = {
        config: { folder_id: 'abc' },
        credentials: { access_token: 'token' },
        connector_state: { cursor: 'xyz' },
      };

      server.use(
        http.get(`${BASE_URL}/sdk/source/:sourceId/sync-config`, () => {
          return HttpResponse.json(mockData);
        })
      );

      const client = new SdkClient(BASE_URL);
      const result = await client.fetchSourceConfig('source-123');

      expect(result).toEqual(mockData);
    });

    it('throws SdkClientError on 404', async () => {
      server.use(
        http.get(`${BASE_URL}/sdk/source/:sourceId/sync-config`, () => {
          return HttpResponse.json(
            { error: 'Source not found' },
            { status: 404 }
          );
        })
      );

      const client = new SdkClient(BASE_URL);
      await expect(
        client.fetchSourceConfig('nonexistent')
      ).rejects.toThrow('Failed to fetch source config: 404');
    });
  });

  describe('error handling', () => {
    it('throws SdkClientError on non-2xx response', async () => {
      server.use(
        http.post(`${BASE_URL}/sdk/events`, () => {
          return HttpResponse.json(
            { error: 'Internal error' },
            { status: 500 }
          );
        })
      );

      const client = new SdkClient(BASE_URL);
      const event: ConnectorEventPayload = {
        type: EventType.DOCUMENT_CREATED,
        sync_run_id: 'sync-123',
        source_id: 'source-456',
        document_id: 'doc-789',
        content_id: 'content-abc',
      };

      await expect(
        client.emitEvent('sync-123', 'source-456', event)
      ).rejects.toThrow('Failed to emit event: 500');
    });
  });
});
