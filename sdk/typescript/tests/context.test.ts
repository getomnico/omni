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

describe('SyncContext.shouldIndexUser', () => {
  function makeCtx(opts: {
    mode?: 'all' | 'whitelist' | 'blacklist';
    whitelist?: string[] | null;
    blacklist?: string[] | null;
  }): SyncContext {
    return new SyncContext(
      new SdkClient(BASE_URL),
      'sync',
      'source',
      undefined,
      SyncMode.INCREMENTAL,
      0,
      0,
      {
        userFilterMode: opts.mode ?? 'all',
        userWhitelist: opts.whitelist ?? null,
        userBlacklist: opts.blacklist ?? null,
      }
    );
  }

  it('returns true for everyone in ALL mode (including empty email)', () => {
    const ctx = makeCtx({ mode: 'all' });
    expect(ctx.shouldIndexUser('alice@example.com')).toBe(true);
    expect(ctx.shouldIndexUser('bob@example.com')).toBe(true);
    expect(ctx.shouldIndexUser('')).toBe(true);
  });

  it('admits only whitelist members in WHITELIST mode (case-insensitive)', () => {
    const ctx = makeCtx({
      mode: 'whitelist',
      whitelist: ['Alice@Example.COM'],
    });
    expect(ctx.shouldIndexUser('alice@example.com')).toBe(true);
    expect(ctx.shouldIndexUser('ALICE@example.com')).toBe(true);
    expect(ctx.shouldIndexUser('bob@example.com')).toBe(false);
    expect(ctx.shouldIndexUser('')).toBe(false);
  });

  it('rejects only blacklist members in BLACKLIST mode (case-insensitive)', () => {
    const ctx = makeCtx({
      mode: 'blacklist',
      blacklist: ['ex@example.com'],
    });
    expect(ctx.shouldIndexUser('ex@example.com')).toBe(false);
    expect(ctx.shouldIndexUser('EX@example.com')).toBe(false);
    expect(ctx.shouldIndexUser('alice@example.com')).toBe(true);
    expect(ctx.shouldIndexUser('')).toBe(false);
  });

  it('treats null lists as empty', () => {
    const wl = makeCtx({ mode: 'whitelist', whitelist: null });
    expect(wl.shouldIndexUser('alice@example.com')).toBe(false);
    const bl = makeCtx({ mode: 'blacklist', blacklist: null });
    expect(bl.shouldIndexUser('alice@example.com')).toBe(true);
  });
});

describe('SyncContext.incrementUpdated', () => {
  it('POSTs the count to /sdk/sync/:id/updated', async () => {
    const calls: Array<{ syncRunId: string; count: number }> = [];
    server.use(
      http.post(
        `${BASE_URL}/sdk/sync/:syncRunId/updated`,
        async ({ request, params }) => {
          const body = (await request.json()) as { count: number };
          calls.push({
            syncRunId: params.syncRunId as string,
            count: body.count,
          });
          return HttpResponse.json({ status: 'ok' });
        }
      )
    );

    const ctx = new SyncContext(
      new SdkClient(BASE_URL),
      'sync-9',
      'source-9'
    );

    await ctx.incrementUpdated(3);
    await ctx.incrementUpdated(5);

    expect(calls).toEqual([
      { syncRunId: 'sync-9', count: 3 },
      { syncRunId: 'sync-9', count: 5 },
    ]);
  });

  it('propagates HTTP errors to the caller', async () => {
    server.use(
      http.post(`${BASE_URL}/sdk/sync/:syncRunId/updated`, () =>
        HttpResponse.text('boom', { status: 500 })
      )
    );
    const ctx = new SyncContext(
      new SdkClient(BASE_URL),
      'sync-10',
      'source-10'
    );
    await expect(ctx.incrementUpdated(1)).rejects.toThrow(
      /Failed to increment updated/
    );
  });
});

describe('SyncContext.complete', () => {
  it('persists newState via updateConnectorState before the status flip', async () => {
    let stateUpdate: { sourceId: string; body: unknown } | null = null;
    let completeCalled = false;
    let completeAfterStateUpdate = false;

    server.use(
      http.post(`${BASE_URL}/sdk/events/batch`, () =>
        HttpResponse.json({ success: true })
      ),
      http.put(
        `${BASE_URL}/sdk/source/:sourceId/connector-state`,
        async ({ request, params }) => {
          stateUpdate = {
            sourceId: params.sourceId as string,
            body: await request.json(),
          };
          return HttpResponse.json({ success: true });
        }
      ),
      http.post(`${BASE_URL}/sdk/sync/:syncRunId/complete`, () => {
        completeCalled = true;
        completeAfterStateUpdate = stateUpdate !== null;
        return HttpResponse.json({ status: 'ok' });
      })
    );

    const ctx = new SyncContext(
      new SdkClient(BASE_URL),
      'sync-c',
      'source-c'
    );
    await ctx.complete({ last_sync_at: '2026-04-28T00:00:00Z' });

    expect(stateUpdate).toEqual({
      sourceId: 'source-c',
      body: { last_sync_at: '2026-04-28T00:00:00Z' },
    });
    expect(completeCalled).toBe(true);
    expect(completeAfterStateUpdate).toBe(true);
  });

  it('skips the state update when no newState is provided', async () => {
    let stateUpdated = false;

    server.use(
      http.post(`${BASE_URL}/sdk/events/batch`, () =>
        HttpResponse.json({ success: true })
      ),
      http.put(`${BASE_URL}/sdk/source/:sourceId/connector-state`, () => {
        stateUpdated = true;
        return HttpResponse.json({ success: true });
      }),
      http.post(`${BASE_URL}/sdk/sync/:syncRunId/complete`, () =>
        HttpResponse.json({ status: 'ok' })
      )
    );

    const ctx = new SyncContext(
      new SdkClient(BASE_URL),
      'sync-d',
      'source-d'
    );
    await ctx.complete();

    expect(stateUpdated).toBe(false);
  });
});

describe('SyncContext.sourceType', () => {
  it('exposes source_type passed via the optional bag', () => {
    const ctx = new SyncContext(
      new SdkClient(BASE_URL),
      'sync',
      'source',
      undefined,
      SyncMode.INCREMENTAL,
      0,
      0,
      { sourceType: 'linear' }
    );
    expect(ctx.sourceType).toBe('linear');
  });

  it('defaults to null when not provided', () => {
    const ctx = new SyncContext(
      new SdkClient(BASE_URL),
      'sync',
      'source'
    );
    expect(ctx.sourceType).toBeNull();
  });
});
