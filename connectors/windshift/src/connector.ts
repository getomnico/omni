import {
  Connector,
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
  WindshiftSourceConfig,
  WindshiftSyncState,
} from "./types.js";

const logger = getLogger("windshift");
const CHECKPOINT_INTERVAL = 100;

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

function windshiftPublicBaseUrl(): string | undefined {
  return normalizedEnvUrl("WINDSHIFT_BASE_URL");
}

function windshiftTransportBaseUrl(): string | undefined {
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
  // Returns undefined when WINDSHIFT_BASE_URL isn't set; the SDK then
  // skips MCP discovery and the connector falls back to read-only sync.
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

  // Windshift 0.8.3 exposes a public-client DCR endpoint. Omni registers
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
    const publicBaseUrl = windshiftPublicBaseUrl();
    const transportBaseUrl = windshiftTransportBaseUrl();
    if (!publicBaseUrl || !transportBaseUrl) {
      await ctx.fail(
        "Connector container is missing WINDSHIFT_BASE_URL env var",
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

    const client = new WindshiftApiClient(
      transportBaseUrl,
      accessToken,
    );

    const isIncremental = ctx.syncMode === SyncMode.INCREMENTAL;
    const lastSyncAt = isIncremental ? state?.last_sync_at : undefined;
    const cutoff = lastSyncAt ? new Date(lastSyncAt).getTime() : null;
    let docsSinceCheckpoint = 0;

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

      for (const workspace of workspaces) {
        if (ctx.isCancelled()) {
          await ctx.fail("Cancelled by user");
          return;
        }
        logger.info(
          `Syncing items for workspace: ${workspace.name} (${workspace.key})`,
        );

        let stoppedEarly = false;
        for await (const item of client.fetchItems(workspace.id)) {
          if (ctx.isCancelled()) {
            await ctx.fail("Cancelled by user");
            return;
          }

          // Server has no updated_since filter — sort=updated_at&order=desc lets us stop when
          // we cross the cutoff. See plan: a Windshift-side updated_since param is the long-term fix.
          if (cutoff !== null && new Date(item.updated_at).getTime() < cutoff) {
            stoppedEarly = true;
            break;
          }

          await ctx.incrementScanned();
          try {
            const comments = await client.fetchItemComments(item.id);
            const content = generateItemContent(item, comments);
            const contentId = await ctx.contentStorage.save(
              content,
              "text/markdown",
            );
            const doc = mapItemToDocument(
              item,
              comments,
              contentId,
              publicBaseUrl,
              sourceOwnerEmail,
            );
            if (isIncremental) {
              await ctx.emitUpdated(doc);
            } else {
              await ctx.emit(doc);
            }
            docsSinceCheckpoint++;
            if (docsSinceCheckpoint >= CHECKPOINT_INTERVAL) {
              await ctx.saveState({ last_sync_at: new Date().toISOString() });
              docsSinceCheckpoint = 0;
            }
          } catch (e) {
            const eid = `windshift:item:${item.id}`;
            logger.warn(`Error processing ${eid}: ${e}`);
            ctx.emitError(eid, String(e));
          }
        }

        if (stoppedEarly) {
          logger.info(
            `Reached incremental cutoff for workspace ${workspace.key}, moving on`,
          );
        }
      }

      await ctx.complete({ last_sync_at: new Date().toISOString() });
      logger.info(
        `Sync completed: ${ctx.documentsScanned} scanned, ${ctx.documentsEmitted} emitted`,
      );
    } catch (e) {
      logger.error({ err: e }, "Sync failed with unexpected error");
      await ctx.fail(String(e));
    }
  }
}
