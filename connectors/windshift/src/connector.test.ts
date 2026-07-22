import assert from "node:assert/strict";
import test from "node:test";

import { SyncMode, type SyncContext } from "@getomnico/connector";
import { WindshiftConnector } from "./connector.js";

test("uses the public issuer for browser OAuth and internal server routes", () => {
  const previousPublicUrl = process.env.WINDSHIFT_BASE_URL;
  const previousInternalUrl = process.env.WINDSHIFT_INTERNAL_BASE_URL;
  process.env.WINDSHIFT_BASE_URL = "http://localhost:8080/";
  process.env.WINDSHIFT_INTERNAL_BASE_URL = "http://host.docker.internal:8080/";

  try {
    const connector = new WindshiftConnector();
    assert.deepEqual(connector.mcpServer, {
      transport: "http",
      url: "http://host.docker.internal:8080/mcp",
    });
    assert.equal(
      connector.oauthConfig?.auth_endpoint,
      "http://localhost:8080/oauth/authorize",
    );
    assert.equal(
      connector.oauthConfig?.registration_endpoint,
      "http://host.docker.internal:8080/api/oauth/register",
    );
    assert.equal(
      connector.oauthConfig?.token_endpoint,
      "http://host.docker.internal:8080/api/oauth/token",
    );
    assert.equal(
      connector.oauthConfig?.userinfo_endpoint,
      "http://host.docker.internal:8080/api/oauth/userinfo",
    );
    assert.equal(connector.oauthConfig?.resource, "http://localhost:8080/mcp");
  } finally {
    if (previousPublicUrl === undefined) delete process.env.WINDSHIFT_BASE_URL;
    else process.env.WINDSHIFT_BASE_URL = previousPublicUrl;
    if (previousInternalUrl === undefined)
      delete process.env.WINDSHIFT_INTERNAL_BASE_URL;
    else process.env.WINDSHIFT_INTERNAL_BASE_URL = previousInternalUrl;
  }
});

test("builds MCP authorization from sync and action credential shapes", () => {
  const connector = new WindshiftConnector();

  assert.deepEqual(connector.prepareMcpHeaders({ access_token: "sync-token" }), {
    Authorization: "Bearer sync-token",
  });
  assert.deepEqual(
    connector.prepareMcpHeaders({
      credentials: { access_token: "action-token" },
    }),
    { Authorization: "Bearer action-token" },
  );
  assert.deepEqual(connector.prepareMcpHeaders({}), {});
});

test("full sync ignores an existing incremental checkpoint", async () => {
  const previousPublicUrl = process.env.WINDSHIFT_BASE_URL;
  const previousInternalUrl = process.env.WINDSHIFT_INTERNAL_BASE_URL;
  const previousFetch = globalThis.fetch;
  process.env.WINDSHIFT_BASE_URL = "http://localhost:5111";
  process.env.WINDSHIFT_INTERNAL_BASE_URL = "http://windshift:8080";

  let emitted = 0;
  let updated = 0;
  let scanned = 0;
  let completed = false;
  let emittedPermissions: unknown;

  globalThis.fetch = async (input) => {
    const url = String(input);
    if (url.includes("/workspaces?")) {
      return Response.json({
        data: [{ id: 1, key: "W1", name: "Workspace 1" }],
        pagination: { page: 1, total_pages: 1, has_more: false },
      });
    }
    if (url.includes("/items?")) {
      return Response.json({
        data: [
          {
            id: 2,
            workspace_id: 1,
            workspace_key: "W1",
            workspace_item_number: 2,
            title: "Milestone item",
            milestones: [{ id: 1, name: "0.8.3" }],
            created_at: "2026-01-01T00:00:00.000Z",
            updated_at: "2026-01-02T00:00:00.000Z",
          },
        ],
        pagination: { page: 1, total_pages: 1, has_more: false },
      });
    }
    if (url.includes("/items/2/comments")) return Response.json([]);
    throw new Error(`Unexpected request: ${url}`);
  };

  const ctx = {
    syncMode: SyncMode.FULL,
    isCancelled: () => false,
    incrementScanned: async () => {
      scanned++;
    },
    contentStorage: { save: async () => "content-1" },
    emit: async (document: { permissions?: unknown }) => {
      emitted++;
      emittedPermissions = document.permissions;
    },
    emitUpdated: async () => {
      updated++;
    },
    saveState: async () => {},
    complete: async () => {
      completed = true;
    },
    fail: async (message: string) => {
      throw new Error(message);
    },
    emitError: () => {},
    getUserEmailForSource: async () => "owner@example.com",
    get documentsScanned() {
      return scanned;
    },
    get documentsEmitted() {
      return emitted + updated;
    },
  } as unknown as SyncContext;

  try {
    const connector = new WindshiftConnector();
    connector.bootstrapMcp = async () => {};
    await connector.sync(
      {},
      { access_token: "token" },
      { last_sync_at: "2026-07-01T00:00:00.000Z" },
      ctx,
    );

    assert.equal(scanned, 1);
    assert.equal(emitted, 1);
    assert.equal(updated, 0);
    assert.equal(completed, true);
    assert.deepEqual(emittedPermissions, {
      public: false,
      users: ["owner@example.com"],
      groups: [],
    });
  } finally {
    globalThis.fetch = previousFetch;
    if (previousPublicUrl === undefined) delete process.env.WINDSHIFT_BASE_URL;
    else process.env.WINDSHIFT_BASE_URL = previousPublicUrl;
    if (previousInternalUrl === undefined)
      delete process.env.WINDSHIFT_INTERNAL_BASE_URL;
    else process.env.WINDSHIFT_INTERNAL_BASE_URL = previousInternalUrl;
  }
});
