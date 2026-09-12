import assert from "node:assert/strict";
import test from "node:test";

import { WindshiftApiClient } from "./client.js";

test("item pagination uses REST v2 and normalizes flat fields", async () => {
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
    assert.equal(url.searchParams.has("order"), false);
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
            status_id: 2,
            status_name: "In Progress",
            workspace_name: "Engineering",
            milestones: [{ id: 3, name: "0.8.3" }],
            created_at: "2026-07-21T12:00:00Z",
            updated_at: "2026-07-21T12:00:00Z",
          },
        ],
        pagination: {
          page: Number(page),
          page_size: 100,
          total_items: 2,
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
      "/rest/api/v2/items",
      "/rest/api/v2/items",
    ]);
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("workspace pagination and comment batches use REST v2 response shapes", async () => {
  const originalFetch = globalThis.fetch;
  const paths: string[] = [];
  let batchBody: unknown;

  globalThis.fetch = async (input, init) => {
    const url = new URL(String(input));
    paths.push(url.pathname);
    if (url.pathname.endsWith("/workspaces")) {
      return Response.json({
        data: [{ id: 1, key: "ENG", name: "Engineering" }],
        pagination: {
          page: 1,
          page_size: 100,
          total_items: 1,
          total_pages: 1,
        },
      });
    }
    assert.equal(url.pathname.endsWith("/comments/batch"), true);
    assert.equal(init?.method, "POST");
    batchBody = JSON.parse(String(init?.body));
    return Response.json({
      data: [
        {
          item_id: 7,
          has_more: false,
          comments: [
            {
              id: 9,
              item_id: 7,
              content: "OAuth now works",
              author_id: 2,
              author_name: "Ada Lovelace",
              created_at: "2026-07-21T12:00:00Z",
              updated_at: "2026-07-21T12:00:00Z",
            },
          ],
        },
      ],
    });
  };

  try {
    const client = new WindshiftApiClient("https://windshift.example/", "token");
    assert.deepEqual(await client.fetchWorkspaces(), [
      { id: 1, key: "ENG", name: "Engineering" },
    ]);
    const commentsByItem = await client.fetchCommentsByItemIds([7, 8]);
    assert.deepEqual(batchBody, {
      item_ids: [7, 8],
      page_size: 50,
    });
    assert.deepEqual(commentsByItem.get(7), [
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
    // Items without feed entries still get an empty list.
    assert.deepEqual(commentsByItem.get(8), []);
    assert.deepEqual(paths, [
      "/rest/api/v2/workspaces",
      "/rest/api/v2/comments/batch",
    ]);
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("comment batches chunk item ids at the endpoint cap", async () => {
  const originalFetch = globalThis.fetch;
  const batches: number[][] = [];

  globalThis.fetch = async (_input, init) => {
    batches.push(JSON.parse(String(init?.body)).item_ids);
    return Response.json({ data: [] });
  };

  try {
    const client = new WindshiftApiClient("https://windshift.example", "token");
    const itemIds = Array.from({ length: 501 }, (_, index) => index + 1);
    const commentsByItem = await client.fetchCommentsByItemIds(itemIds);
    assert.equal(batches.length, 2);
    assert.equal(batches[0].length, 500);
    assert.equal(batches[1].length, 1);
    assert.equal(commentsByItem.size, 501);
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("comment batches skip the request for empty input", async () => {
  const originalFetch = globalThis.fetch;
  let called = false;

  globalThis.fetch = async () => {
    called = true;
    return Response.json({ data: [] });
  };

  try {
    const client = new WindshiftApiClient("https://windshift.example", "token");
    assert.deepEqual(await client.fetchCommentsByItemIds([]), new Map());
    assert.equal(called, false);
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("item changes normalize v2 cursors and batch-fetch changed items", async () => {
  const originalFetch = globalThis.fetch;
  const requestedPaths: string[] = [];
  globalThis.fetch = async (input, init) => {
    const url = new URL(String(input));
    requestedPaths.push(`${url.pathname}?${url.searchParams}`);
    if (url.pathname.endsWith("/items/changes")) {
      assert.equal(url.searchParams.get("workspace_id"), "1");
      assert.equal(url.searchParams.get("since"), "10");
      assert.equal(url.searchParams.get("through"), "13");
      assert.equal(url.searchParams.get("limit"), "500");
      return Response.json({
        data: {
          changed_item_ids: [7],
          removed_item_ids: [8],
          next_cursor: 13,
          watermark: 13,
          has_more: false,
          reset_required: false,
        },
      });
    }
    assert.equal(url.pathname.endsWith("/items/batch"), true);
    assert.equal(init?.method, "POST");
    assert.deepEqual(JSON.parse(String(init?.body)), { ids: [7, 9] });
    return Response.json({
      data: [
        {
          id: 7,
          workspace_id: 1,
          workspace_key: "ENG",
          workspace_item_number: 7,
          title: "Changed item",
          created_at: "2026-07-21T12:00:00Z",
          updated_at: "2026-07-21T12:00:00Z",
        },
      ],
    });
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
    assert.deepEqual(requestedPaths, [
      "/rest/api/v2/items/changes?workspace_id=1&limit=500&since=10&through=13",
      "/rest/api/v2/items/batch?",
    ]);
  } finally {
    globalThis.fetch = originalFetch;
  }
});
