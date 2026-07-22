import type {
  WindshiftItem,
  WindshiftPaginatedResponse,
  WindshiftWorkspace,
  WindshiftComment,
} from "./types.js";

const PAGE_SIZE = 100;
const MAX_COMMENTS_PER_ITEM = 50;

function joinUrl(baseUrl: string, path: string): string {
  const trimmed = baseUrl.replace(/\/+$/, "");
  return `${trimmed}/rest/api/v1${path}`;
}

type WindshiftUserResponse = {
  id: number;
  email?: string;
  username?: string;
  full_name?: string;
};

type WindshiftItemResponse = {
  id: number;
  workspace_id: number;
  workspace_key?: string;
  key?: string;
  workspace_item_number?: number;
  title: string;
  description?: string | null;
  status?: { id: number; name: string } | null;
  priority?: { id: number; name: string } | null;
  assignee?: WindshiftUserResponse | null;
  creator?: WindshiftUserResponse | null;
  workspace?: { id: number; name: string; key: string } | null;
  milestones?: Array<{ id: number; name: string }>;
  iteration?: { id: number; name: string } | null;
  created_at: string;
  updated_at: string;
  completed_at?: string | null;
};

type WindshiftCommentResponse = {
  id: number;
  item_id: number;
  content: string;
  author?: WindshiftUserResponse | null;
  created_at: string;
  updated_at: string;
};

function userName(user: WindshiftUserResponse | null | undefined): string | null {
  return user?.full_name || user?.username || user?.email || null;
}

function mapItem(item: WindshiftItemResponse): WindshiftItem {
  return {
    id: item.id,
    workspace_id: item.workspace_id,
    workspace_name: item.workspace?.name ?? null,
    workspace_key: item.workspace_key ?? item.workspace?.key ?? null,
    workspace_item_number: item.workspace_item_number ?? null,
    title: item.title,
    description: item.description ?? null,
    status_id: item.status?.id ?? null,
    status_name: item.status?.name ?? null,
    priority_id: item.priority?.id ?? null,
    priority_name: item.priority?.name ?? null,
    assignee_id: item.assignee?.id ?? null,
    assignee_name: userName(item.assignee),
    assignee_email: item.assignee?.email ?? null,
    creator_id: item.creator?.id ?? null,
    creator_name: userName(item.creator),
    milestones: item.milestones ?? [],
    iteration: item.iteration ?? null,
    created_at: item.created_at,
    updated_at: item.updated_at,
    completed_at: item.completed_at ?? null,
  };
}

function mapComment(comment: WindshiftCommentResponse): WindshiftComment {
  return {
    id: comment.id,
    item_id: comment.item_id,
    user_id: comment.author?.id ?? null,
    user_name: userName(comment.author),
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

  private async request<T>(path: string): Promise<T> {
    const res = await fetch(joinUrl(this.baseUrl, path), {
      headers: {
        Authorization: `Bearer ${this.apiToken}`,
        Accept: "application/json",
      },
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
        limit: String(PAGE_SIZE),
      });
      const response = await this.request<
        WindshiftPaginatedResponse<WindshiftWorkspace>
      >(`/workspaces?${params}`);
      workspaces.push(...response.data);
      if (!response.pagination.has_more) return workspaces;
      page = response.pagination.page + 1;
    }
  }

  async *fetchItems(workspaceId?: number): AsyncGenerator<WindshiftItem> {
    let page = 1;
    while (true) {
      const params = new URLSearchParams({
        page: String(page),
        limit: String(PAGE_SIZE),
        sort: "updated_at",
        order: "desc",
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

  async fetchItemComments(itemId: number): Promise<WindshiftComment[]> {
    const data = await this.request<WindshiftCommentResponse[]>(
      `/items/${itemId}/comments`,
    );
    return data.slice(0, MAX_COMMENTS_PER_ITEM).map(mapComment);
  }

  async getItem(itemId: number): Promise<WindshiftItem> {
    const item = await this.request<WindshiftItemResponse>(`/items/${itemId}`);
    return mapItem(item);
  }

  async transitionItem(itemId: number, toStatusId: number): Promise<unknown> {
    return this.write("POST", `/items/${itemId}/transition`, {
      to_status_id: toStatusId,
    });
  }

  async updateItem(
    itemId: number,
    fields: Record<string, unknown>,
  ): Promise<unknown> {
    return this.write("PUT", `/items/${itemId}`, fields);
  }

  async createItem(fields: Record<string, unknown>): Promise<unknown> {
    return this.write("POST", `/items`, fields);
  }

  private async write(
    method: "POST" | "PUT",
    path: string,
    body: Record<string, unknown>,
  ): Promise<unknown> {
    const res = await fetch(joinUrl(this.baseUrl, path), {
      method,
      headers: {
        Authorization: `Bearer ${this.apiToken}`,
        "Content-Type": "application/json",
        Accept: "application/json",
      },
      body: JSON.stringify(body),
    });
    if (!res.ok) {
      const text = await res.text().catch(() => "");
      throw new Error(
        `Windshift API ${res.status} for ${method} ${path}: ${text.slice(0, 300)}`,
      );
    }
    if (res.status === 204) return null;
    return res.json();
  }
}
