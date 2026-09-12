import type {
  WindshiftItem,
  WindshiftPaginatedResponse,
  WindshiftWorkspace,
  WindshiftComment,
  WindshiftItemChangesResponse,
} from "./types.js";

const PAGE_SIZE = 100;
const CHANGE_PAGE_SIZE = 500;
const MAX_COMMENTS_PER_ITEM = 50;
// The v2 comments/batch endpoint accepts at most 500 item ids per request.
const MAX_COMMENT_BATCH_ITEMS = 500;

function joinUrl(baseUrl: string, path: string): string {
  const trimmed = baseUrl.replace(/\/+$/, "");
  return `${trimmed}/rest/api/v2${path}`;
}

type WindshiftItemResponse = {
  id: number;
  workspace_id: number;
  workspace_item_number?: number;
  key?: string;
  workspace_key?: string;
  workspace_name?: string;
  title: string;
  description?: string | null;
  status_id?: number | null;
  status_name?: string;
  priority_id?: number | null;
  priority_name?: string;
  assignee_id?: number | null;
  assignee_name?: string;
  assignee_email?: string;
  creator_id?: number | null;
  creator_name?: string;
  creator_email?: string;
  milestones?: Array<{ id: number; name: string }>;
  iteration_id?: number | null;
  iteration_name?: string;
  created_at: string;
  updated_at: string;
  completed_at?: string | null;
};

type WindshiftCommentResponse = {
  id: number;
  item_id: number;
  author_id?: number | null;
  author_name?: string;
  author_email?: string;
  content: string;
  created_at: string;
  updated_at: string;
};

type WindshiftItemChangesResponseV2 = {
  changed_item_ids: number[];
  removed_item_ids: number[];
  next_cursor: number;
  watermark: number;
  has_more: boolean;
  reset_required: boolean;
};

type WindshiftCommentBatchEntry = {
  item_id: number;
  comments: WindshiftCommentResponse[];
  has_more: boolean;
};

type WindshiftEnvelope<T> = { data: T };

function userName(
  name: string | undefined,
  email: string | undefined,
): string | null {
  return name || email || null;
}

function mapItem(item: WindshiftItemResponse): WindshiftItem {
  return {
    id: item.id,
    workspace_id: item.workspace_id,
    workspace_name: item.workspace_name ?? null,
    workspace_key: item.workspace_key ?? null,
    workspace_item_number: item.workspace_item_number ?? null,
    title: item.title,
    description: item.description ?? null,
    status_id: item.status_id ?? null,
    status_name: item.status_name ?? null,
    priority_id: item.priority_id ?? null,
    priority_name: item.priority_name ?? null,
    assignee_id: item.assignee_id ?? null,
    assignee_name: userName(item.assignee_name, item.assignee_email),
    assignee_email: item.assignee_email ?? null,
    creator_id: item.creator_id ?? null,
    creator_name: userName(item.creator_name, item.creator_email),
    milestones: item.milestones ?? [],
    iteration:
      item.iteration_id != null && item.iteration_name
        ? { id: item.iteration_id, name: item.iteration_name }
        : null,
    created_at: item.created_at,
    updated_at: item.updated_at,
    completed_at: item.completed_at ?? null,
  };
}

function mapComment(comment: WindshiftCommentResponse): WindshiftComment {
  return {
    id: comment.id,
    item_id: comment.item_id,
    user_id: comment.author_id ?? null,
    user_name: userName(comment.author_name, comment.author_email),
    body: comment.content,
    created_at: comment.created_at,
    updated_at: comment.updated_at,
  };
}

export class WindshiftApiClient {
  constructor(
    private readonly baseUrl: string,
    private readonly apiToken: string,
  ) {}

  private async request<T>(
    path: string,
    options: { method?: "GET" | "POST"; body?: Record<string, unknown> } = {},
  ): Promise<T> {
    const headers: Record<string, string> = {
      Authorization: `Bearer ${this.apiToken}`,
      Accept: "application/json",
    };
    if (options.body) headers["Content-Type"] = "application/json";
    const res = await fetch(joinUrl(this.baseUrl, path), {
      method: options.method,
      headers,
      body: options.body ? JSON.stringify(options.body) : undefined,
    });
    if (!res.ok) {
      const body = await res.text().catch(() => "");
      throw new Error(
        `Windshift API ${res.status} for ${path}: ${body.slice(0, 200)}`,
      );
    }
    return (await res.json()) as T;
  }

  async fetchWorkspaces(): Promise<WindshiftWorkspace[]> {
    const workspaces: WindshiftWorkspace[] = [];
    let page = 1;
    while (true) {
      const params = new URLSearchParams({
        page: String(page),
        page_size: String(PAGE_SIZE),
      });
      const response = await this.request<
        WindshiftPaginatedResponse<WindshiftWorkspace>
      >(`/workspaces?${params}`);
      workspaces.push(...response.data);
      if (response.pagination.page >= response.pagination.total_pages) {
        return workspaces;
      }
      page = response.pagination.page + 1;
    }
  }

  async *fetchItems(workspaceId?: number): AsyncGenerator<WindshiftItem> {
    let page = 1;
    while (true) {
      const params = new URLSearchParams({
        page: String(page),
        page_size: String(PAGE_SIZE),
        sort: "key",
      });
      if (workspaceId !== undefined) {
        params.set("workspace_id", String(workspaceId));
      }
      const res = await this.request<
        WindshiftPaginatedResponse<WindshiftItemResponse>
      >(
        `/items?${params}`,
      );
      for (const item of res.data) {
        yield mapItem(item);
      }
      if (
        res.data.length === 0 ||
        res.pagination.page >= res.pagination.total_pages
      ) {
        return;
      }
      page = res.pagination.page + 1;
    }
  }

  async fetchItemChanges(
    workspaceId: number,
    since?: string,
    through?: string,
  ): Promise<WindshiftItemChangesResponse> {
    const params = new URLSearchParams({
      workspace_id: String(workspaceId),
      limit: String(CHANGE_PAGE_SIZE),
    });
    if (since !== undefined) params.set("since", since);
    if (through !== undefined) params.set("through", through);
    const response = await this.request<
      WindshiftEnvelope<WindshiftItemChangesResponseV2>
    >(`/items/changes?${params}`);
    const changes: WindshiftItemChangesResponse["changes"] = [
      ...response.data.changed_item_ids.map((item_id) => ({
        item_id,
        change_type: "upsert" as const,
      })),
      ...response.data.removed_item_ids.map((item_id) => ({
        item_id,
        change_type: "delete" as const,
      })),
    ];
    return {
      changes,
      next_cursor: String(response.data.next_cursor),
      watermark: String(response.data.watermark),
      has_more: response.data.has_more,
      reset_required: response.data.reset_required,
    };
  }

  async fetchItemsByIds(itemIds: number[]): Promise<WindshiftItem[]> {
    if (itemIds.length === 0) return [];
    const response = await this.request<
      WindshiftEnvelope<WindshiftItemResponse[]>
    >("/items/batch", { method: "POST", body: { ids: itemIds } });
    return response.data.map(mapItem);
  }

  // One request per chunk instead of one per item: a full sync of N items
  // issues ceil(N / 100) comment requests instead of N.
  async fetchCommentsByItemIds(
    itemIds: number[],
  ): Promise<Map<number, WindshiftComment[]>> {
    const commentsByItem = new Map<number, WindshiftComment[]>();
    for (const id of itemIds) {
      commentsByItem.set(id, []);
    }
    for (let start = 0; start < itemIds.length; start += MAX_COMMENT_BATCH_ITEMS) {
      const chunk = itemIds.slice(start, start + MAX_COMMENT_BATCH_ITEMS);
      const response = await this.request<
        WindshiftEnvelope<WindshiftCommentBatchEntry[]>
      >("/comments/batch", {
        method: "POST",
        body: { item_ids: chunk, page_size: MAX_COMMENTS_PER_ITEM },
      });
      for (const entry of response.data) {
        commentsByItem.set(
          entry.item_id,
          entry.comments.map(mapComment),
        );
      }
    }
    return commentsByItem;
  }
}
