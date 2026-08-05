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
    // The manifest advertises the operator-configured private route so the web
    // layer can allow RFC1918 resolution for that exact origin at use time.
    assert.equal(
      connector.oauthConfig?.internal_base_url,
      "http://host.docker.internal:8080",
    );
  } finally {
    if (previousPublicUrl === undefined) delete process.env.WINDSHIFT_BASE_URL;
    else process.env.WINDSHIFT_BASE_URL = previousPublicUrl;
    if (previousInternalUrl === undefined)
      delete process.env.WINDSHIFT_INTERNAL_BASE_URL;
    else process.env.WINDSHIFT_INTERNAL_BASE_URL = previousInternalUrl;
  }
});

test("omits the internal route marker without WINDSHIFT_INTERNAL_BASE_URL", () => {
  const previousPublicUrl = process.env.WINDSHIFT_BASE_URL;
  const previousInternalUrl = process.env.WINDSHIFT_INTERNAL_BASE_URL;
  process.env.WINDSHIFT_BASE_URL = "https://windshift.example.com/";
  delete process.env.WINDSHIFT_INTERNAL_BASE_URL;

  try {
    const connector = new WindshiftConnector();
    assert.equal(connector.oauthConfig?.internal_base_url, undefined);
    assert.equal(
      connector.oauthConfig?.token_endpoint,
      "https://windshift.example.com/api/oauth/token",
    );
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

  assert.deepEqual(
    connector.prepareMcpHeaders({ access_token: "sync-token" }),
    {
      Authorization: "Bearer sync-token",
    },
  );
  assert.deepEqual(
    connector.prepareMcpHeaders({
      credentials: { access_token: "action-token" },
    }),
    { Authorization: "Bearer action-token" },
  );
  assert.deepEqual(connector.prepareMcpHeaders({}), {});
});

test("full sync checkpoints only after indexing visible items", async () => {
  const previousPublicUrl = process.env.WINDSHIFT_BASE_URL;
  const previousInternalUrl = process.env.WINDSHIFT_INTERNAL_BASE_URL;
  const previousFetch = globalThis.fetch;
  process.env.WINDSHIFT_BASE_URL = "http://localhost:5111";
  process.env.WINDSHIFT_INTERNAL_BASE_URL = "http://windshift:8080";

  let emitted = 0;
  let scanned = 0;
  let completed = false;
  let emittedPermissions: unknown;
  let completedState: unknown;

  globalThis.fetch = async (input) => {
    const url = String(input);
    if (url.includes("/workspaces?")) {
      return Response.json({
        data: [{ id: 1, key: "W1", name: "Workspace 1" }],
        pagination: { page: 1, total_pages: 1, has_more: false },
      });
    }
    if (url.includes("/items/changes?")) {
      return Response.json({
        changes: [],
        next_cursor: "5",
        watermark: "5",
        has_more: false,
        reset_required: false,
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
    if (url.includes("/items/2/comments")) {
      return Response.json({ data: [] });
    }
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
    complete: async (state: unknown) => {
      completed = true;
      completedState = state;
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
      return emitted;
    },
  } as unknown as SyncContext;

  try {
    const connector = new WindshiftConnector();
    assert.deepEqual(connector.syncModes, ["full", "incremental"]);
    connector.bootstrapMcp = async () => {};
    await connector.sync({}, { access_token: "token" }, null, ctx);

    assert.equal(scanned, 1);
    assert.equal(emitted, 1);
    assert.equal(completed, true);
    assert.deepEqual(completedState, {
      workspace_cursors: { "1": "5" },
    });
    assert.deepEqual(emittedPermissions, {
      public: false,
      users: ["owner@example.com"],
      groups: [],
    });

    completed = false;
    let failedMessage = "";
    await connector.sync({}, { access_token: "token" }, null, {
      ...ctx,
      contentStorage: {
        save: async () => {
          throw new Error("storage unavailable");
        },
      },
      complete: async () => {
        completed = true;
      },
      fail: async (message: string) => {
        failedMessage = message;
      },
    } as unknown as SyncContext);
    assert.match(failedMessage, /storage unavailable/);
    assert.equal(completed, false);
  } finally {
    globalThis.fetch = previousFetch;
    if (previousPublicUrl === undefined) delete process.env.WINDSHIFT_BASE_URL;
    else process.env.WINDSHIFT_BASE_URL = previousPublicUrl;
    if (previousInternalUrl === undefined)
      delete process.env.WINDSHIFT_INTERNAL_BASE_URL;
    else process.env.WINDSHIFT_INTERNAL_BASE_URL = previousInternalUrl;
  }
});

test("incremental sync consumes change pages and emits deletions", async () => {
  const previousPublicUrl = process.env.WINDSHIFT_BASE_URL;
  const previousInternalUrl = process.env.WINDSHIFT_INTERNAL_BASE_URL;
  const previousFetch = globalThis.fetch;
  process.env.WINDSHIFT_BASE_URL = "http://localhost:5111";
  process.env.WINDSHIFT_INTERNAL_BASE_URL = "http://windshift:8080";

  const requestedPaths: string[] = [];
  let changePage = 0;
  let failureMode = false;
  globalThis.fetch = async (input) => {
    const url = new URL(String(input));
    requestedPaths.push(`${url.pathname}?${url.searchParams}`);
    if (url.pathname.endsWith("/workspaces")) {
      return Response.json({
        data: [{ id: 1, key: "W1", name: "Workspace 1" }],
        pagination: { page: 1, total_pages: 1, has_more: false },
      });
    }
    if (url.pathname.endsWith("/items/changes")) {
      if (failureMode) {
        return Response.json({
          changes: [{ item_id: 9, change_type: "upsert" }],
          next_cursor: "13",
          watermark: "13",
          has_more: false,
          reset_required: false,
        });
      }
      changePage++;
      if (changePage === 1) {
        assert.equal(url.searchParams.get("since"), "10");
        assert.equal(url.searchParams.has("through"), false);
        return Response.json({
          changes: [{ item_id: 7, change_type: "upsert" }],
          next_cursor: "11",
          watermark: "12",
          has_more: true,
          reset_required: false,
        });
      }
      assert.equal(url.searchParams.get("since"), "11");
      assert.equal(url.searchParams.get("through"), "12");
      return Response.json({
        changes: [{ item_id: 8, change_type: "delete" }],
        next_cursor: "12",
        watermark: "12",
        has_more: false,
        reset_required: false,
      });
    }
    if (url.pathname.endsWith("/items/batch")) {
      if (failureMode) {
        return Response.json([
          {
            id: 9,
            workspace_id: 1,
            workspace_key: "W1",
            workspace_item_number: 9,
            title: "Broken item",
            created_at: "2026-01-01T00:00:00.000Z",
            updated_at: "2026-01-02T00:00:00.000Z",
          },
        ]);
      }
      assert.equal(url.searchParams.get("ids"), "7");
      return Response.json([
        {
          id: 7,
          workspace_id: 1,
          workspace_key: "W1",
          workspace_item_number: 7,
          title: "Changed item",
          created_at: "2026-01-01T00:00:00.000Z",
          updated_at: "2026-01-02T00:00:00.000Z",
        },
      ]);
    }
    if (url.pathname.endsWith("/items/7/comments")) {
      return Response.json({ data: [] });
    }
    if (url.pathname.endsWith("/items/9/comments")) {
      return Response.json({ data: [] });
    }
    throw new Error(`Unexpected request: ${url}`);
  };

  let updated = 0;
  const deleted: string[] = [];
  const savedStates: unknown[] = [];
  let completedState: unknown;
  const ctx = {
    syncMode: SyncMode.INCREMENTAL,
    isCancelled: () => false,
    incrementScanned: async () => {},
    contentStorage: { save: async () => "content-7" },
    emitUpdated: async () => {
      updated++;
    },
    emitDeleted: async (externalId: string) => {
      deleted.push(externalId);
    },
    saveState: async (state: unknown) => {
      savedStates.push(structuredClone(state));
    },
    complete: async (state: unknown) => {
      completedState = state;
    },
    fail: async (message: string) => {
      throw new Error(message);
    },
    emitError: () => {},
    getUserEmailForSource: async () => "owner@example.com",
    get documentsScanned() {
      return 1;
    },
    get documentsEmitted() {
      return updated;
    },
  } as unknown as SyncContext;

  try {
    const connector = new WindshiftConnector();
    connector.bootstrapMcp = async () => {};
    await connector.sync(
      {},
      { access_token: "token" },
      { workspace_cursors: { "1": "10" } },
      ctx,
    );

    assert.equal(updated, 1);
    assert.deepEqual(deleted, ["windshift:item:8"]);
    assert.deepEqual(savedStates, [
      { workspace_cursors: { "1": "11" } },
      { workspace_cursors: { "1": "12" } },
    ]);
    assert.deepEqual(completedState, {
      workspace_cursors: { "1": "12" },
    });
    assert.equal(
      requestedPaths.some((path) => path.startsWith("/rest/api/v1/items?")),
      false,
    );

    failureMode = true;
    let failedMessage = "";
    let savedAfterFailure = false;
    let completedAfterFailure = false;
    const failedCtx = {
      ...ctx,
      contentStorage: {
        save: async () => {
          throw new Error("storage unavailable");
        },
      },
      saveState: async () => {
        savedAfterFailure = true;
      },
      complete: async () => {
        completedAfterFailure = true;
      },
      fail: async (message: string) => {
        failedMessage = message;
      },
    } as unknown as SyncContext;
    await connector.sync(
      {},
      { access_token: "token" },
      { workspace_cursors: { "1": "12" } },
      failedCtx,
    );
    assert.match(failedMessage, /storage unavailable/);
    assert.equal(savedAfterFailure, false);
    assert.equal(completedAfterFailure, false);
  } finally {
    globalThis.fetch = previousFetch;
    if (previousPublicUrl === undefined) delete process.env.WINDSHIFT_BASE_URL;
    else process.env.WINDSHIFT_BASE_URL = previousPublicUrl;
    if (previousInternalUrl === undefined)
      delete process.env.WINDSHIFT_INTERNAL_BASE_URL;
    else process.env.WINDSHIFT_INTERNAL_BASE_URL = previousInternalUrl;
  }
});
