import type {
  Document,
  DocumentMetadata,
  DocumentPermissions,
} from "@getomnico/connector";
import type {
  WindshiftAttributes,
  WindshiftComment,
  WindshiftItem,
} from "./types.js";

const MAX_CONTENT_LENGTH = 100_000;

function truncate(content: string): string {
  if (content.length > MAX_CONTENT_LENGTH) {
    return content.slice(0, MAX_CONTENT_LENGTH) + "\n... (truncated)";
  }
  return content;
}

function itemUrl(baseUrl: string, item: WindshiftItem): string | undefined {
  const root = baseUrl.replace(/\/+$/, "");
  if (item.workspace_key && item.workspace_item_number != null) {
    return `${root}/workspace/${encodeURIComponent(item.workspace_key)}/item/${item.workspace_item_number}`;
  }
  return `${root}/workspaces/${item.workspace_id}/items/${item.id}`;
}

function itemIdentifier(item: WindshiftItem): string | undefined {
  if (!item.workspace_key || item.workspace_item_number == null)
    return undefined;
  return `${item.workspace_key}-${item.workspace_item_number}`;
}

function itemPermissions(sourceOwnerEmail: string): DocumentPermissions {
  return {
    public: false,
    users: [sourceOwnerEmail.trim().toLowerCase()],
    groups: [],
  };
}

export function mapItemToDocument(
  item: WindshiftItem,
  comments: WindshiftComment[],
  contentId: string,
  baseUrl: string,
  sourceOwnerEmail: string,
): Document {
  const identifier = itemIdentifier(item);
  const attributes: WindshiftAttributes = {
    status: item.status_name ?? null,
    priority: item.priority_name ?? null,
    assignee: item.assignee_name ?? null,
    assignee_email: item.assignee_email ?? null,
    workspace: item.workspace_name ?? null,
    identifier,
    milestone:
      item.milestones && item.milestones.length > 0
        ? item.milestones.map((milestone) => milestone.name).join(", ")
        : null,
    iteration: item.iteration?.name ?? null,
  };

  const pathParts = [item.workspace_name, identifier].filter(
    Boolean,
  ) as string[];

  const metadata: DocumentMetadata = {
    author: item.creator_name ?? undefined,
    created_at: item.created_at,
    updated_at: item.updated_at,
    content_type: "item",
    url: itemUrl(baseUrl, item),
    mime_type: "text/markdown",
    path: pathParts.join(" / ") || undefined,
  };

  const title = identifier ? `${identifier} - ${item.title}` : item.title;

  return {
    external_id: `windshift:item:${item.id}`,
    title,
    content_id: contentId,
    metadata,
    permissions: itemPermissions(sourceOwnerEmail),
    attributes,
  };
}

export function generateItemContent(
  item: WindshiftItem,
  comments: WindshiftComment[],
): string {
  const lines: string[] = [];
  const identifier = itemIdentifier(item);
  lines.push(identifier ? `${identifier}: ${item.title}` : item.title);

  const headerParts: string[] = [];
  if (item.status_name) headerParts.push(`Status: ${item.status_name}`);
  if (item.priority_name) headerParts.push(`Priority: ${item.priority_name}`);
  if (item.workspace_name)
    headerParts.push(`Workspace: ${item.workspace_name}`);
  if (headerParts.length > 0) lines.push(headerParts.join(" | "));

  if (item.assignee_name) lines.push(`Assignee: ${item.assignee_name}`);
  if (item.creator_name) lines.push(`Created by: ${item.creator_name}`);
  if (item.milestones && item.milestones.length > 0) {
    lines.push(
      `Milestones: ${item.milestones.map((milestone) => milestone.name).join(", ")}`,
    );
  }
  if (item.iteration) lines.push(`Iteration: ${item.iteration.name}`);

  lines.push("");
  if (item.description) lines.push(item.description);

  if (comments.length > 0) {
    lines.push("");
    lines.push("--- Comments ---");
    for (const comment of comments) {
      const dateStr = comment.created_at.split("T")[0];
      lines.push(`${comment.user_name ?? "Unknown"} (${dateStr}):`);
      if (comment.body) lines.push(comment.body);
      lines.push("");
    }
  }

  return truncate(lines.join("\n"));
}
