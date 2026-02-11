import { describe, it, expect, beforeAll, afterAll, vi } from 'vitest';
import request from 'supertest';
import { http, HttpResponse } from 'msw';
import { setupServer } from 'msw/node';
import { Connector } from '../src/connector.js';
import { createServer } from '../src/server.js';
import type { SyncContext } from '../src/context.js';

const MANAGER_URL = 'http://test-connector-manager:8080';

class MockConnector extends Connector {
  name = 'mock-connector';
  version = '1.0.0';
  syncModes = ['full', 'incremental'];

  syncFn: ((ctx: SyncContext) => Promise<void>) | null = null;

  async sync(
    _sourceConfig: Record<string, unknown>,
    _credentials: Record<string, unknown>,
    _state: Record<string, unknown> | null,
    ctx: SyncContext
  ): Promise<void> {
    if (this.syncFn) {
      await this.syncFn(ctx);
    }
  }
}

const mockServer = setupServer(
  http.get(`${MANAGER_URL}/sdk/source/:sourceId/sync-config`, () =>
    HttpResponse.json({
      config: { folder_id: 'test-folder' },
      credentials: { access_token: 'test-token' },
      connector_state: { cursor: 'test-cursor' },
    })
  ),
  http.post(`${MANAGER_URL}/sdk/events`, () => HttpResponse.json({ success: true })),
  http.post(`${MANAGER_URL}/sdk/content`, () => HttpResponse.json({ content_id: 'content-123' })),
  http.post(`${MANAGER_URL}/sdk/sync/:id/heartbeat`, () => HttpResponse.json({ success: true })),
  http.post(`${MANAGER_URL}/sdk/sync/:id/scanned`, () => HttpResponse.json({ success: true })),
  http.post(`${MANAGER_URL}/sdk/sync/:id/complete`, () => HttpResponse.json({ success: true })),
  http.post(`${MANAGER_URL}/sdk/sync/:id/fail`, () => HttpResponse.json({ success: true }))
);

beforeAll(() => {
  vi.stubEnv('CONNECTOR_MANAGER_URL', MANAGER_URL);
  mockServer.listen({ onUnhandledRequest: 'bypass' });
});

afterAll(() => {
  vi.unstubAllEnvs();
  mockServer.close();
});

describe('Connector Server', () => {
  describe('GET /health', () => {
    it('returns healthy status', async () => {
      const connector = new MockConnector();
      const app = createServer(connector);

      const response = await request(app).get('/health');

      expect(response.status).toBe(200);
      expect(response.body).toEqual({
        status: 'healthy',
        service: 'mock-connector',
      });
    });
  });

  describe('GET /manifest', () => {
    it('returns connector manifest', async () => {
      const connector = new MockConnector();
      const app = createServer(connector);

      const response = await request(app).get('/manifest');

      expect(response.status).toBe(200);
      expect(response.body).toEqual({
        name: 'mock-connector',
        version: '1.0.0',
        sync_modes: ['full', 'incremental'],
        actions: [],
      });
    });
  });

  describe('POST /sync', () => {
    it('returns 400 for invalid request body', async () => {
      const connector = new MockConnector();
      const app = createServer(connector);

      const response = await request(app)
        .post('/sync')
        .send({ invalid: 'data' });

      expect(response.status).toBe(400);
      expect(response.body.status).toBe('error');
    });

    it('fetches config from API and returns started', async () => {
      const connector = new MockConnector();
      const app = createServer(connector);

      const response = await request(app)
        .post('/sync')
        .send({
          sync_run_id: 'sync-123',
          source_id: 'source-456',
          sync_mode: 'full',
        });

      expect(response.status).toBe(200);
      expect(response.body.status).toBe('started');
    });
  });

  describe('POST /cancel', () => {
    it('returns not_found for unknown sync', async () => {
      const connector = new MockConnector();
      const app = createServer(connector);

      const response = await request(app)
        .post('/cancel')
        .send({ sync_run_id: 'unknown-sync' });

      expect(response.status).toBe(200);
      expect(response.body).toEqual({ status: 'not_found' });
    });

    it('returns 400 for invalid request body', async () => {
      const connector = new MockConnector();
      const app = createServer(connector);

      const response = await request(app)
        .post('/cancel')
        .send({ invalid: 'data' });

      expect(response.status).toBe(400);
    });
  });

  describe('POST /action', () => {
    it('returns not supported for unknown action', async () => {
      const connector = new MockConnector();
      const app = createServer(connector);

      const response = await request(app)
        .post('/action')
        .send({
          action: 'unknown_action',
          params: {},
          credentials: {},
        });

      expect(response.status).toBe(200);
      expect(response.body).toEqual({
        status: 'error',
        error: 'Action not supported: unknown_action',
      });
    });

    it('returns 400 for invalid request body', async () => {
      const connector = new MockConnector();
      const app = createServer(connector);

      const response = await request(app)
        .post('/action')
        .send({ invalid: 'data' });

      expect(response.status).toBe(400);
    });
  });
});
