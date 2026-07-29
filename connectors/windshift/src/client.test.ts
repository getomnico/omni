import assert from "node:assert/strict";
import test from "node:test";

import { WindshiftApiClient } from "./client.js";

test("item pagination uses REST v1 and normalizes nested fields", async () => {
  const originalFetch = globalThis.fetch;
  const requestedPages: string[] = [];
  const requestedPaths: string[] = [];

  globalThis.fetch = async (input, init) => {
    const url = new URL(String(input));
    const page = url.searchParams.get("page") ?? "1";
    requestedPages.push(page);
    requestedPaths.push(url.pathname);
    assert.equal(
      init?.headers && (init.headers as Record<string, string>).Authorization,
      "Bearer token",
    );
    assert.equal(url.searchParams.get("sort"), "key");
    assert.equal(url.searchParams.get("order"), "asc");
    assert.equal(url.searchParams.get("workspace_id"), "1");
    return new Response(
      JSON.stringify({
        data: [
          {
            id: Number(page),
            workspace_id: 1,
            workspace_key: "ENG",
            workspace_item_number: Number(page),
            title: `Item ${page}`,
            status: { id: 2, name: "In Progress" },
            workspace: { id: 1, key: "ENG", name: "Engineering" },
            milestones: [{ id: 3, name: "0.8.3" }],
            created_at: "2026-07-21T12:00:00Z",
            updated_at: "2026-07-21T12:00:00Z",
          },
        ],
        pagination: {
          page: Number(page),
          limit: 100,
          total: 2,
          total_pages: 2,
        },
      }),
      { status: 200, headers: { "Content-Type": "application/json" } },
    );
  };

  try {
    const client = new WindshiftApiClient("https://windshift.example", "token");
    const itemIds: number[] = [];
    for await (const item of client.fetchItems(1)) {
      itemIds.push(item.id);
    }

    assert.deepEqual(itemIds, [1, 2]);
    assert.deepEqual(requestedPages, ["1", "2"]);
    assert.deepEqual(requestedPaths, [
      "/rest/api/v1/items",
      "/rest/api/v1/items",
    ]);
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("workspace pagination and comments use REST v1 response shapes", async () => {
  const originalFetch = globalThis.fetch;
  const paths: string[] = [];

  globalThis.fetch = async (input) => {
    const url = new URL(String(input));
    paths.push(url.pathname);
    if (url.pathname.endsWith("/workspaces")) {
      return Response.json({
        data: [{ id: 1, key: "ENG", name: "Engineering" }],
        pagination: {
          page: 1,
          limit: 100,
          total: 1,
          total_pages: 1,
          has_more: false,
        },
      });
    }
    assert.equal(url.searchParams.get("page"), "1");
    assert.equal(url.searchParams.get("limit"), "50");
    assert.equal(url.searchParams.get("order"), "desc");
    return Response.json({
      data: [
        {
          id: 9,
          item_id: 7,
          content: "OAuth now works",
          author: { id: 2, full_name: "Ada Lovelace" },
          created_at: "2026-07-21T12:00:00Z",
          updated_at: "2026-07-21T12:00:00Z",
        },
      ],
      pagination: {
        page: 1,
        limit: 50,
        total: 1,
        total_pages: 1,
        has_more: false,
      },
    });
  };

  try {
    const client = new WindshiftApiClient("https://windshift.example/", "token");
    assert.deepEqual(await client.fetchWorkspaces(), [
      { id: 1, key: "ENG", name: "Engineering" },
    ]);
    assert.deepEqual(await client.fetchItemComments(7), [
      {
        id: 9,
        item_id: 7,
        user_id: 2,
        user_name: "Ada Lovelace",
        body: "OAuth now works",
        created_at: "2026-07-21T12:00:00Z",
        updated_at: "2026-07-21T12:00:00Z",
      },
    ]);
    assert.deepEqual(paths, [
      "/rest/api/v1/workspaces",
      "/rest/api/v1/items/7/comments",
    ]);
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("item changes use string cursors and batch-fetch changed items", async () => {
  const originalFetch = globalThis.fetch;
  const requestedPaths: string[] = [];
  globalThis.fetch = async (input) => {
    const url = new URL(String(input));
    requestedPaths.push(`${url.pathname}?${url.searchParams}`);
    if (url.pathname.endsWith("/items/changes")) {
      assert.equal(url.searchParams.get("workspace_id"), "1");
      assert.equal(url.searchParams.get("since"), "10");
      assert.equal(url.searchParams.get("through"), "13");
      assert.equal(url.searchParams.get("limit"), "500");
      return Response.json({
        changes: [
          { item_id: 7, change_type: "upsert" },
          { item_id: 8, change_type: "delete" },
        ],
        next_cursor: "13",
        watermark: "13",
        has_more: false,
        reset_required: false,
      });
    }
    assert.equal(url.pathname.endsWith("/items/batch"), true);
    assert.equal(url.searchParams.get("ids"), "7,9");
    return Response.json([
      {
        id: 7,
        workspace_id: 1,
        workspace_key: "ENG",
        workspace_item_number: 7,
        title: "Changed item",
        created_at: "2026-07-21T12:00:00Z",
        updated_at: "2026-07-21T12:00:00Z",
      },
    ]);
  };

  try {
    const client = new WindshiftApiClient("https://windshift.example", "token");
    const changes = await client.fetchItemChanges(1, "10", "13");
    assert.deepEqual(changes.changes, [
      { item_id: 7, change_type: "upsert" },
      { item_id: 8, change_type: "delete" },
    ]);
    assert.equal(changes.next_cursor, "13");

    const items = await client.fetchItemsByIds([7, 9]);
    assert.deepEqual(
      items.map((item) => item.id),
      [7],
    );
    assert.equal(requestedPaths.length, 2);
  } finally {
    globalThis.fetch = originalFetch;
  }
});
