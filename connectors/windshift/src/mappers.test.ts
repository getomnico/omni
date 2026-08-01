import assert from "node:assert/strict";
import test from "node:test";

import { generateItemContent, mapItemToDocument } from "./mappers.js";
import type { WindshiftItem } from "./types.js";

test("maps item links to Windshift's stable workspace-key route", () => {
  const item: WindshiftItem = {
    id: 42,
    workspace_id: 7,
    workspace_key: "ENG",
    workspace_item_number: 12,
    title: "OAuth conformance",
    created_at: "2026-07-21T12:00:00Z",
    updated_at: "2026-07-21T12:00:00Z",
  };

  const document = mapItemToDocument(
    item,
    [],
    "content-1",
    "https://windshift.example/base/",
    "Owner@Example.COM",
  );

  assert.equal(
    document.metadata?.url,
    "https://windshift.example/base/workspace/ENG/item/12",
  );
  assert.deepEqual(document.permissions, {
    public: false,
    users: ["owner@example.com"],
    groups: [],
  });
});

test("indexes milestone and iteration context", () => {
  const item: WindshiftItem = {
    id: 42,
    workspace_id: 7,
    workspace_name: "Engineering",
    workspace_key: "ENG",
    workspace_item_number: 12,
    title: "OAuth conformance",
    milestones: [{ id: 3, name: "0.8.3" }],
    iteration: { id: 5, name: "Sprint 14" },
    created_at: "2026-07-21T12:00:00Z",
    updated_at: "2026-07-21T12:00:00Z",
  };

  const document = mapItemToDocument(
    item,
    [],
    "content-1",
    "https://windshift.example",
    "owner@example.com",
  );
  assert.equal(document.attributes?.milestone, "0.8.3");
  assert.equal(document.attributes?.iteration, "Sprint 14");
  assert.match(generateItemContent(item, []), /Milestones: 0\.8\.3/);
  assert.match(generateItemContent(item, []), /Iteration: Sprint 14/);
});
