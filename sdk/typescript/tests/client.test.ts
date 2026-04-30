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
    it('calls correct endpoint with { count: 1 } body', async () => {
      let calledPath = '';
      let capturedBody: unknown;

      server.use(
        http.post(`${BASE_URL}/sdk/sync/:id/scanned`, async ({ params, request }) => {
          calledPath = `/sdk/sync/${params.id}/scanned`;
          capturedBody = await request.json();
          return HttpResponse.json({ success: true });
        })
      );

      const client = new SdkClient(BASE_URL);
      await client.incrementScanned('sync-run-abc');

      expect(calledPath).toBe('/sdk/sync/sync-run-abc/scanned');
      // Body MUST be sent — connector-manager's Json<T> extractor rejects
      // an empty body with 400 even though the underlying struct has a
      // serde default for `count`.
      expect(capturedBody).toEqual({ count: 1 });
    });
  });

  describe('complete (status-flip-only post-7c21fd10)', () => {
    it('POSTs an empty body to /sdk/sync/:id/complete', async () => {
      let capturedBody: unknown;
      let capturedMethod: string | null = null;

      server.use(
        http.post(`${BASE_URL}/sdk/sync/sync-1/complete`, async ({ request }) => {
          capturedMethod = request.method;
          const text = await request.text();
          capturedBody = text === '' ? null : JSON.parse(text);
          return HttpResponse.json({ status: 'ok' });
        })
      );

      const client = new SdkClient(BASE_URL);
      await client.complete('sync-1');

      expect(capturedMethod).toBe('POST');
      // Body is either absent or an empty object — server ignores it either way.
      // Plan picks "no body sent" to match Rust SDK's complete signature.
      expect(capturedBody).toBeNull();
    });
  });

  describe('incrementUpdated', () => {
    it('POSTs {count} to /sdk/sync/:id/updated', async () => {
      let captured: unknown;
      server.use(
        http.post(`${BASE_URL}/sdk/sync/sync-1/updated`, async ({ request }) => {
          captured = await request.json();
          return HttpResponse.json({ status: 'ok' });
        })
      );
      const client = new SdkClient(BASE_URL);
      await client.incrementUpdated('sync-1', 7);
      expect(captured).toEqual({ count: 7 });
    });

    it('throws SdkClientError on non-OK response', async () => {
      server.use(
        http.post(`${BASE_URL}/sdk/sync/sync-1/updated`, () =>
          HttpResponse.text('boom', { status: 500 })
        )
      );
      const client = new SdkClient(BASE_URL);
      await expect(client.incrementUpdated('sync-1', 1)).rejects.toThrow(
        /Failed to increment updated/
      );
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

      expect(result).toMatchObject(mockData);
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

    it('defaults missing user-filter fields to ALL/null', async () => {
      server.use(
        http.get(`${BASE_URL}/sdk/source/source-min/sync-config`, () =>
          HttpResponse.json({
            config: {},
            credentials: {},
            connector_state: null,
          })
        )
      );

      const client = new SdkClient(BASE_URL);
      const cfg = await client.fetchSourceConfig('source-min');

      expect(cfg.user_filter_mode).toBe('all');
      expect(cfg.user_whitelist).toBeNull();
      expect(cfg.user_blacklist).toBeNull();
      expect(cfg.source_type).toBeNull();
    });

    it('rejects an unknown user_filter_mode at parse time', async () => {
      server.use(
        http.get(`${BASE_URL}/sdk/source/source-bad/sync-config`, () =>
          HttpResponse.json({
            config: {},
            credentials: {},
            connector_state: null,
            user_filter_mode: 'invalid_mode',
          })
        )
      );

      const client = new SdkClient(BASE_URL);
      await expect(client.fetchSourceConfig('source-bad')).rejects.toThrow();
    });

    it('parses a fully populated sync-config', async () => {
      server.use(
        http.get(`${BASE_URL}/sdk/source/source-full/sync-config`, () =>
          HttpResponse.json({
            config: { workspaces: ['ws1'] },
            credentials: { token: 'redacted' },
            connector_state: { cursor: 'abc' },
            source_type: 'linear',
            user_filter_mode: 'whitelist',
            user_whitelist: ['alice@example.com', 'bob@example.com'],
            user_blacklist: null,
          })
        )
      );

      const client = new SdkClient(BASE_URL);
      const cfg = await client.fetchSourceConfig('source-full');

      expect(cfg.config).toEqual({ workspaces: ['ws1'] });
      expect(cfg.credentials).toEqual({ token: 'redacted' });
      expect(cfg.connector_state).toEqual({ cursor: 'abc' });
      expect(cfg.source_type).toBe('linear');
      expect(cfg.user_filter_mode).toBe('whitelist');
      expect(cfg.user_whitelist).toEqual(['alice@example.com', 'bob@example.com']);
      expect(cfg.user_blacklist).toBeNull();
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
