import {
  Connector,
  SdkClient,
  SyncMode,
  type SyncContext,
  type SearchOperator,
  type ActionDefinition,
  getLogger,
} from "@getomnico/connector";
import type { McpServer } from "@getomnico/connector";
import { WindshiftApiClient } from "./client.js";
import { generateItemContent, mapItemToDocument } from "./mappers.js";
import type {
  WindshiftCredentials,
  WindshiftItem,
  WindshiftSourceConfig,
  WindshiftSyncState,
} from "./types.js";

const logger = getLogger("windshift");

const READ_SCOPES = [
  "mcp:access",
  "items:read",
  // Item updates and comments are core Windshift actions. Omni still requires
  // explicit approval for every write tool invocation.
  "items:write",
  "workspaces:read",
  "custom-fields:read",
  "users:read",
  "milestones:read",
  "iterations:read",
  "actions:read",
  "pages:read",
  "tests:read",
  "time:read",
];

const WRITE_SCOPES = [
  ...READ_SCOPES,
  "items:delete",
  "actions:write",
  "pages:write",
  "pages:delete",
  "tests:write",
  "time:write",
];

function normalizedEnvUrl(name: string): string | undefined {
  const url = process.env[name];
  if (!url) return undefined;
  return url.replace(/\/+$/, "");
}

// The Windshift server is an admin-managed setting: the base URLs are
// configured in the UI and stored in the connector_configs table (served to
// us by connector-manager). The WINDSHIFT_BASE_URL / WINDSHIFT_INTERNAL_BASE_URL
// env vars remain as a fallback for deployments that predate the UI setting.
//
// The SDK re-registers our manifest every 30s, so once this cache is
// populated the OAuth config the web layer reads reflects the UI setting
// without a container restart.
interface WindshiftServerConfig {
  base_url?: string;
  internal_base_url?: string;
}

let windshiftServerConfig: WindshiftServerConfig | null = null;
let windshiftServerConfigPromise: Promise<WindshiftServerConfig | null> | null =
  null;

function parseWindshiftServerConfig(value: unknown): WindshiftServerConfig {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    return {};
  }
  const record = value as Record<string, unknown>;
  const baseUrl =
    typeof record.base_url === "string" ? record.base_url.trim() : "";
  const internalBaseUrl =
    typeof record.internal_base_url === "string"
      ? record.internal_base_url.trim()
      : "";
  return {
    base_url: baseUrl || undefined,
    internal_base_url: internalBaseUrl || undefined,
  };
}

/// Fetch the Windshift server config from connector-manager's
/// connector_configs table. Never throws: any failure (connector-manager
/// unreachable, no config row, malformed payload) just clears the cache and
/// leaves the env-var fallback in place.
///
/// `force` skips the memoized in-flight/previous promise so a periodic caller
/// can re-read the table and pick up UI changes.
export async function refreshWindshiftServerConfig(
  force = false,
): Promise<WindshiftServerConfig | null> {
  if (!force && windshiftServerConfigPromise) {
    return windshiftServerConfigPromise;
  }
  windshiftServerConfigPromise = (async () => {
    try {
      // 10s cap so an unreachable connector-manager doesn't stall startup.
      const client = new SdkClient(undefined, 10_000);
      const config = await client.getConnectorConfig("windshift");
      windshiftServerConfig = parseWindshiftServerConfig(config);
      logger.info(
        { hasBaseUrl: !!windshiftServerConfig.base_url },
        "Loaded Windshift server config from connector-manager",
      );
    } catch (err) {
      logger.warn(
        { err },
        "Failed to load Windshift server config from connector-manager; falling back to env vars",
      );
      windshiftServerConfig = null;
    }
    return windshiftServerConfig;
  })();
  return windshiftServerConfigPromise;
}

function windshiftPublicBaseUrl(): string | undefined {
  if (windshiftServerConfig?.base_url) {
    return windshiftServerConfig.base_url.replace(/\/+$/, "");
  }
  return normalizedEnvUrl("WINDSHIFT_BASE_URL");
}

function windshiftTransportBaseUrl(): string | undefined {
  const config = windshiftServerConfig;
  if (config?.internal_base_url) {
    return config.internal_base_url.replace(/\/+$/, "");
  }
  if (config?.base_url) {
    return config.base_url.replace(/\/+$/, "");
  }
  return (
    normalizedEnvUrl("WINDSHIFT_INTERNAL_BASE_URL") ?? windshiftPublicBaseUrl()
  );
}

function windshiftAccessToken(
  credentials: WindshiftCredentials,
): string | undefined {
  return credentials?.access_token ?? credentials?.credentials?.access_token;
}

export class WindshiftConnector extends Connector<
  WindshiftSourceConfig,
  WindshiftCredentials,
  WindshiftSyncState
> {
  readonly name = "windshift";
  readonly version = "1.0.0";
  readonly sourceTypes = ["windshift"];

  get description(): string {
    return "Connect to Windshift items across your workspaces";
  }

  get displayName(): string {
    return "Windshift";
  }

  readonly syncModes = ["full", "incremental"];

  // No static actions — the action surface comes from Windshift's `/mcp`
  // server via the HTTP MCP transport below. The Omni connector-manager
  // discovers the tools after first sync (per the SDK's bootstrapMcp flow)
  // and surfaces them as connector actions automatically.
  readonly actions: ActionDefinition[] = [];

  readonly searchOperators: SearchOperator[] = [
    { operator: "status", attribute_key: "status", value_type: "text" },
    { operator: "priority", attribute_key: "priority", value_type: "text" },
    { operator: "assignee", attribute_key: "assignee", value_type: "person" },
    { operator: "workspace", attribute_key: "workspace", value_type: "text" },
    { operator: "milestone", attribute_key: "milestone", value_type: "text" },
    { operator: "iteration", attribute_key: "iteration", value_type: "text" },
  ];

  readonly attributesSchema = {
    type: "object",
    properties: {
      status: { type: "string" },
      priority: { type: "string" },
      assignee: { type: "string" },
      assignee_email: { type: "string", format: "email" },
      workspace: { type: "string" },
      identifier: { type: "string" },
      milestone: { type: "string" },
      iteration: { type: "string" },
    },
  };

  readonly extraSchema = {
    type: "object",
    properties: {
      workspace_keys: {
        type: "array",
        items: { type: "string" },
        description: "Restrict sync to these workspace keys (omit for all)",
      },
    },
  };

  // Wrap Windshift's existing /mcp server (Streamable HTTP, bearer auth)
  // so every Windshift MCP tool — list_items, transition_item, add_comment,
  // etc. — becomes an Omni connector action without per-tool wiring here.
  // Returns undefined when no Windshift base URL is configured (neither the
  // admin UI setting nor the env fallback); the SDK then skips MCP discovery
  // and the connector falls back to read-only sync.
  get mcpServer(): McpServer | undefined {
    const url = windshiftTransportBaseUrl();
    if (!url) return undefined;
    return { transport: "http", url: `${url}/mcp` };
  }

  // Bridges OAuth credentials to the Authorization header the remote MCP
  // server expects. Omni's web layer wrote the token after the user
  // completed the per-user OAuth flow. Sync dispatches the token directly;
  // action dispatch wraps it in Omni's ServiceCredential envelope.
  prepareMcpHeaders(credentials: WindshiftCredentials): Record<string, string> {
    const accessToken = windshiftAccessToken(credentials);
    if (!accessToken) return {};
    return {
      Authorization: `Bearer ${accessToken}`,
    };
  }

  // Windshift exposes a public-client DCR endpoint. Omni registers
  // itself automatically, uses S256 PKCE, and binds every issued token to
  // this exact MCP resource. No administrator-managed client secret is needed.
  override get oauthConfig() {
    const publicBaseUrl = windshiftPublicBaseUrl();
    const transportBaseUrl = windshiftTransportBaseUrl();
    if (!publicBaseUrl || !transportBaseUrl) return undefined;
    return {
      provider: "windshift",
      // The browser must use the public issuer. Registration, token exchange,
      // and user-info requests are server-to-server and may need the private
      // route when Omni and Windshift run in separate containers.
      auth_endpoint: `${publicBaseUrl}/oauth/authorize`,
      token_endpoint: `${transportBaseUrl}/api/oauth/token`,
      registration_endpoint: `${transportBaseUrl}/api/oauth/register`,
      userinfo_endpoint: `${transportBaseUrl}/api/oauth/userinfo`,
      userinfo_email_field: "email",
      identity_scopes: [],
      scopes: {
        windshift: {
          read: READ_SCOPES,
          write: WRITE_SCOPES,
        },
      },
      extra_auth_params: { resource: `${publicBaseUrl}/mcp` },
      scope_separator: " ",
      token_endpoint_auth_method: "none" as const,
      resource: `${publicBaseUrl}/mcp`,
    };
  }

  async sync(
    config: WindshiftSourceConfig,
    credentials: WindshiftCredentials,
    state: WindshiftSyncState | null,
    ctx: SyncContext,
  ): Promise<void> {
    // Ensure we've attempted to load the admin-configured Windshift server
    // (e.g. when the first sync arrives before the startup fetch finished).
    await refreshWindshiftServerConfig();
    const publicBaseUrl = windshiftPublicBaseUrl();
    const transportBaseUrl = windshiftTransportBaseUrl();
    if (!publicBaseUrl || !transportBaseUrl) {
      await ctx.fail(
        "Windshift base URL is not configured. Set it in Admin > Integrations > Windshift server.",
      );
      return;
    }
    const accessToken = windshiftAccessToken(credentials);
    if (!accessToken) {
      await ctx.fail("Missing 'access_token' in credentials");
      return;
    }

    let sourceOwnerEmail: string;
    try {
      sourceOwnerEmail = await ctx.getUserEmailForSource();
    } catch (e) {
      logger.error({ err: e }, "Source owner lookup failed");
      await ctx.fail(`Failed to resolve source owner: ${e}`);
      return;
    }

    // The SDK exposes MCP discovery but does not invoke it automatically.
    // Bootstrap once credentials are available; failures are logged by the
    // SDK and do not block document sync.
    await this.bootstrapMcp(credentials);

    const client = new WindshiftApiClient(transportBaseUrl, accessToken);

    let allWorkspaces;
    try {
      allWorkspaces = await client.fetchWorkspaces();
      logger.info(
        `Starting Windshift sync (${allWorkspaces.length} workspaces visible)`,
      );
    } catch (e) {
      logger.error({ err: e }, "Authentication / workspace fetch failed");
      await ctx.fail(`Authentication failed: ${e}`);
      return;
    }

    try {
      const workspaceFilter = config.workspace_keys;
      const workspaces = workspaceFilter
        ? allWorkspaces.filter((w: { key: string }) =>
            workspaceFilter.includes(w.key),
          )
        : allWorkspaces;
      const incremental = ctx.syncMode === SyncMode.INCREMENTAL;
      const nextState: WindshiftSyncState = {
        workspace_cursors: incremental
          ? { ...(state?.workspace_cursors ?? {}) }
          : {},
      };

      for (const workspace of workspaces) {
        if (ctx.isCancelled()) {
          await ctx.fail("Cancelled by user");
          return;
        }
        logger.info(
          `Syncing items for workspace: ${workspace.name} (${workspace.key})`,
        );

        const workspaceKey = String(workspace.id);
        const cursor = nextState.workspace_cursors[workspaceKey];
        if (incremental && cursor !== undefined) {
          await this.syncWorkspaceChanges(
            client,
            workspace.id,
            cursor,
            nextState,
            publicBaseUrl,
            sourceOwnerEmail,
            ctx,
          );
          continue;
        }

        // Capture the watermark before crawling. Changes committed while the
        // crawl is running remain above it and are replayed incrementally.
        const primed = await client.fetchItemChanges(workspace.id);
        await this.syncWorkspaceFull(
          client,
          workspace.id,
          incremental,
          publicBaseUrl,
          sourceOwnerEmail,
          ctx,
        );
        nextState.workspace_cursors[workspaceKey] = primed.watermark;
        if (incremental) await ctx.saveState(nextState);
      }

      await ctx.complete(nextState);
      logger.info(
        `Sync completed: ${ctx.documentsScanned} scanned, ${ctx.documentsEmitted} emitted`,
      );
    } catch (e) {
      logger.error({ err: e }, "Sync failed with unexpected error");
      await ctx.fail(String(e));
    }
  }

  private async syncWorkspaceFull(
    client: WindshiftApiClient,
    workspaceId: number,
    emitAsUpdate: boolean,
    publicBaseUrl: string,
    sourceOwnerEmail: string,
    ctx: SyncContext,
  ): Promise<void> {
    for await (const item of client.fetchItems(workspaceId)) {
      if (ctx.isCancelled()) throw new Error("Cancelled by user");
      try {
        await this.emitItem(
          client,
          item,
          emitAsUpdate,
          publicBaseUrl,
          sourceOwnerEmail,
          ctx,
        );
      } catch (e) {
        const externalId = `windshift:item:${item.id}`;
        logger.warn(`Error processing ${externalId}: ${e}`);
        ctx.emitError(externalId, String(e));
        throw e;
      }
    }
  }

  private async syncWorkspaceChanges(
    client: WindshiftApiClient,
    workspaceId: number,
    initialCursor: string,
    state: WindshiftSyncState,
    publicBaseUrl: string,
    sourceOwnerEmail: string,
    ctx: SyncContext,
  ): Promise<void> {
    let cursor = initialCursor;
    let through: string | undefined;
    while (true) {
      if (ctx.isCancelled()) throw new Error("Cancelled by user");
      const page = await client.fetchItemChanges(workspaceId, cursor, through);
      if (page.reset_required) {
        throw new Error(
          `Windshift change cursor for workspace ${workspaceId} is no longer valid; run a full sync`,
        );
      }
      through ??= page.watermark;

      const latestChanges = new Map<number, "upsert" | "delete">();
      for (const change of page.changes) {
        latestChanges.set(change.item_id, change.change_type);
      }
      const upsertIds = [...latestChanges]
        .filter(([, changeType]) => changeType === "upsert")
        .map(([itemId]) => itemId);
      const items = await client.fetchItemsByIds(upsertIds);
      const itemsById = new Map(items.map((item) => [item.id, item]));

      for (const [itemId, changeType] of latestChanges) {
        if (ctx.isCancelled()) throw new Error("Cancelled by user");
        if (changeType === "delete") {
          await ctx.emitDeleted(`windshift:item:${itemId}`);
          continue;
        }
        const item = itemsById.get(itemId);
        if (!item) {
          // The item was deleted or became invisible after the change event.
          await ctx.emitDeleted(`windshift:item:${itemId}`);
          continue;
        }
        try {
          await this.emitItem(
            client,
            item,
            true,
            publicBaseUrl,
            sourceOwnerEmail,
            ctx,
          );
        } catch (e) {
          const externalId = `windshift:item:${item.id}`;
          ctx.emitError(externalId, String(e));
          throw new Error(`Failed to process ${externalId}: ${e}`);
        }
      }

      cursor = page.next_cursor;
      state.workspace_cursors[String(workspaceId)] = cursor;
      await ctx.saveState(state);
      if (!page.has_more) return;
    }
  }

  private async emitItem(
    client: WindshiftApiClient,
    item: WindshiftItem,
    update: boolean,
    publicBaseUrl: string,
    sourceOwnerEmail: string,
    ctx: SyncContext,
  ): Promise<void> {
    await ctx.incrementScanned();
    const comments = await client.fetchItemComments(item.id);
    const content = generateItemContent(item, comments);
    const contentId = await ctx.contentStorage.save(content, "text/markdown");
    const doc = mapItemToDocument(
      item,
      comments,
      contentId,
      publicBaseUrl,
      sourceOwnerEmail,
    );
    if (update) await ctx.emitUpdated(doc);
    else await ctx.emit(doc);
  }
}
