export type WindshiftSyncState = {
  last_sync_at: string;
};

export type WindshiftSourceConfig = {
  workspace_keys?: string[];
};

// Credentials are written by Omni's generic OAuth dispatcher
// (`web/src/lib/server/oauth/connectorOAuth.ts`) after the user completes
// the authorization-code flow against Windshift's `/oauth/authorize` +
// `/api/oauth/token`. The connector container reaches Windshift through
// WINDSHIFT_BASE_URL (process env), not through credentials — one Omni
// install pointing at one Windshift instance.
export type WindshiftTokenCredentials = {
  access_token?: string;
  refresh_token?: string;
  token_type?: string; // 'Bearer'
  expires_at?: string; // ISO8601
};

// Sync requests contain the token fields directly. Action requests contain
// Omni's ServiceCredential envelope, whose provider payload is nested under
// `credentials`.
export type WindshiftCredentials = WindshiftTokenCredentials & {
  credentials?: WindshiftTokenCredentials;
};

export type WindshiftAttributes = {
  status?: string | null;
  priority?: string | null;
  assignee?: string | null;
  assignee_email?: string | null;
  workspace?: string | null;
  identifier?: string;
  milestone?: string | null;
  iteration?: string | null;
};

export type WindshiftMilestone = {
  id: number;
  name: string;
};

export type WindshiftIteration = {
  id: number;
  name: string;
};

export type WindshiftItem = {
  id: number;
  workspace_id: number;
  workspace_name?: string | null;
  workspace_key?: string | null;
  workspace_item_number?: number | null;
  title: string;
  description?: string | null;
  status_id?: number | null;
  status_name?: string | null;
  priority_id?: number | null;
  priority_name?: string | null;
  assignee_id?: number | null;
  assignee_name?: string | null;
  assignee_email?: string | null;
  creator_id?: number | null;
  creator_name?: string | null;
  milestones?: WindshiftMilestone[];
  iteration?: WindshiftIteration | null;
  created_at: string;
  updated_at: string;
  completed_at?: string | null;
};

export type WindshiftPaginatedResponse<T> = {
  data: T[];
  pagination: {
    page: number;
    limit: number;
    total: number;
    total_pages: number;
    has_more: boolean;
  };
};

export type WindshiftWorkspace = {
  id: number;
  key: string;
  name: string;
};

export type WindshiftComment = {
  id: number;
  item_id: number;
  user_id?: number | null;
  user_name?: string | null;
  body: string;
  created_at: string;
  updated_at: string;
};
