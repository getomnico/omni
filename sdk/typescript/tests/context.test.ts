import { describe, it, expect, beforeAll, afterAll, afterEach, vi } from 'vitest';
import { http, HttpResponse } from 'msw';
import { setupServer } from 'msw/node';
import { SdkClient } from '../src/client.js';
import { SyncContext } from '../src/context.js';
import {
  EventType,
  SyncMode,
  type ConnectorEventPayload,
  type Document,
} from '../src/models.js';

const BASE_URL = 'http://test-cm:8080';
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

function captureEvents(): ConnectorEventPayload[] {
  const captured: ConnectorEventPayload[] = [];
  server.use(
    http.post(`${BASE_URL}/sdk/events`, async ({ request }) => {
      const body = (await request.json()) as { event: ConnectorEventPayload };
      captured.push(body.event);
      return HttpResponse.json({ success: true });
    }),
    http.post(`${BASE_URL}/sdk/events/batch`, async ({ request }) => {
      const body = (await request.json()) as { events: ConnectorEventPayload[] };
      captured.push(...body.events);
      return HttpResponse.json({ success: true });
    })
  );
  return captured;
}

describe('SyncContext.emit — title shim', () => {
  it('copies Document.title into metadata.title when metadata.title is missing', async () => {
    const captured = captureEvents();
    const ctx = new SyncContext(
      new SdkClient(BASE_URL),
      'sync-1',
      'source-1',
      undefined,
      SyncMode.REALTIME // size=1, flush-on-emit
    );
    const doc: Document = {
      external_id: 'ext-1',
      title: 'Real Title',
      content_id: 'content-1',
      metadata: { content_type: 'card', mime_type: 'text/markdown' },
    };

    await ctx.emit(doc);

    expect(captured).toHaveLength(1);
    expect(captured[0].metadata?.title).toBe('Real Title');
  });

  it('does not mutate the caller\'s Document', async () => {
    captureEvents();
    const ctx = new SyncContext(
      new SdkClient(BASE_URL),
      'sync-2',
      'source-2',
      undefined,
      SyncMode.REALTIME
    );
    const originalMetadata = { content_type: 'card' };
    const doc: Document = {
      external_id: 'ext-2',
      title: 'Title',
      content_id: 'content-2',
      metadata: originalMetadata,
    };

    await ctx.emit(doc);

    expect(originalMetadata).toEqual({ content_type: 'card' });
    expect(doc.metadata).toBe(originalMetadata);
  });

  it('preserves an explicit metadata.title set by the connector', async () => {
    const captured = captureEvents();
    const ctx = new SyncContext(
      new SdkClient(BASE_URL),
      'sync-3',
      'source-3',
      undefined,
      SyncMode.REALTIME
    );
    const doc: Document = {
      external_id: 'ext-3',
      title: 'Wire Title',
      content_id: 'content-3',
      metadata: { title: 'Explicit Metadata Title', content_type: 'card' },
    };

    await ctx.emit(doc);

    expect(captured[0].metadata?.title).toBe('Explicit Metadata Title');
  });

  it('handles missing metadata by creating it with title set', async () => {
    const captured = captureEvents();
    const ctx = new SyncContext(
      new SdkClient(BASE_URL),
      'sync-4',
      'source-4',
      undefined,
      SyncMode.REALTIME
    );
    const doc: Document = {
      external_id: 'ext-4',
      title: 'Bare Doc',
      content_id: 'content-4',
    };

    await ctx.emit(doc);

    expect(captured[0].metadata?.title).toBe('Bare Doc');
  });

  it('emitUpdated applies the same shim', async () => {
    const captured = captureEvents();
    const ctx = new SyncContext(
      new SdkClient(BASE_URL),
      'sync-5',
      'source-5',
      undefined,
      SyncMode.REALTIME
    );
    const doc: Document = {
      external_id: 'ext-5',
      title: 'Updated Title',
      content_id: 'content-5',
      metadata: { content_type: 'document' },
    };

    await ctx.emitUpdated(doc);

    expect(captured[0].type).toBe(EventType.DOCUMENT_UPDATED);
    expect(captured[0].metadata?.title).toBe('Updated Title');
  });
});
