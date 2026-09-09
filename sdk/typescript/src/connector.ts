import type { SyncContext } from './context.js';
import type { McpAdapter, McpServer } from './mcp-adapter.js';
import { MCP_AUTH_REQUIRED_MESSAGE } from './mcp-adapter.js';
import type {
  ConnectorManifest,
  ActionDefinition,
  SearchOperator,
  OAuthManifestConfig,
  Source,
  OAuthCredentialReadyRequest,
} from './models.js';
import { ActionResponse } from './models.js';
import { createServer } from './server.js';
import { getLogger } from './logger.js';

const logger = getLogger('sdk:connector');

export interface ServeOptions {
  port?: number;
  host?: string;
}

export abstract class Connector<
  TConfig extends Record<string, unknown> = Record<string, unknown>,
  TCredentials extends Record<string, unknown> = Record<string, unknown>,
  TState extends Record<string, unknown> = Record<string, unknown>,
> {
  abstract readonly name: string;
  abstract readonly version: string;
  abstract readonly sourceTypes: string[];

  private _mcpAdapter: unknown | null = null;

  get displayName(): string {
    return this.name;
  }

  get description(): string {
    return '';
  }

  readonly syncModes: string[] = ['full'];
  readonly actions: ActionDefinition[] = [];
  readonly searchOperators: SearchOperator[] = [];
  readonly extraSchema?: Record<string, unknown>;
  readonly attributesSchema?: Record<string, unknown>;

  /**
   * Return declarative OAuth2 config if this connector supports Omni-managed
   * OAuth. DCR/public-client providers should set `registration_endpoint` and
   * `token_endpoint_auth_method: 'none'`; set `resource` only when the provider
   * requires an OAuth resource indicator for the target API/MCP server.
   */
  get oauthConfig(): OAuthManifestConfig | undefined {
    return undefined;
  }

  /**
   * Return MCP server config (stdio or Streamable HTTP) if this connector
   * supports MCP. Override this getter to enable MCP support.
   * Requires @modelcontextprotocol/sdk as a dependency.
   *
   * @example
   * get mcpServer(): McpServer {
   *   return { transport: 'stdio', command: 'github-mcp-server', args: ['stdio'] };
   * }
   *
   * @example
   * get mcpServer(): McpServer {
   *   return { transport: 'http', url: 'https://api.example.com/mcp' };
   * }
   */
  get mcpServer(): McpServer | undefined {
    return undefined;
  }

  async getMcpAdapter(): Promise<McpAdapter | undefined> {
    if (this._mcpAdapter !== null) {
      return this._mcpAdapter as McpAdapter;
    }
    const server = this.mcpServer;
    if (!server) {
      return undefined;
    }
    const { McpAdapter } = await import('./mcp-adapter.js');
    this._mcpAdapter = new McpAdapter(server);
    return this._mcpAdapter as McpAdapter;
  }

  private async discoverMcpCatalog(credentials: TCredentials): Promise<boolean> {
    const adapter = await this.getMcpAdapter();
    if (!adapter) {
      return false;
    }
    const { env, headers } = this.prepareMcpAuth(credentials);
    await adapter.discover(env, headers);
    return adapter.hasCachedCatalog();
  }

  /**
   * Discover MCP tools/resources/prompts and cache them. Called when
   * credentials first become available (e.g., during initial sync).
   */
  async bootstrapMcp(credentials: TCredentials): Promise<void> {
    logger.info('Bootstrapping MCP: discovering tools');
    try {
      await this.discoverMcpCatalog(credentials);
    } catch (err) {
      logger.warn({ err }, 'MCP bootstrap failed');
    }
  }

  /**
   * React to a newly stored OAuth credential. MCP-backed connectors use it to
   * restore their authenticated catalog after OAuth or a connector restart.
   */
  async oauthCredentialReady(
    request: OAuthCredentialReadyRequest
  ): Promise<boolean> {
    logger.info('Refreshing MCP catalog after OAuth credential update');
    try {
      return await this.discoverMcpCatalog(request.credentials as TCredentials);
    } catch (err) {
      const adapter = await this.getMcpAdapter();
      adapter?.clearCachedCatalog();
      logger.warn({ err }, 'OAuth credential-ready MCP refresh failed');
      return false;
    }
  }

  /**
   * Return whether an MCP failure requires the acting user's OAuth reconnect.
   * Connectors with provider-specific authentication errors can override this
   * without exposing credentials in an HTTP response; failures carrying the
   * generic auth-status marker are already recognized by the SDK.
   */
  mcpAuthenticationError(_message: string): boolean {
    return false;
  }

  /**
   * Build the stable 412 `needs_user_auth` HTTP response for a terminal MCP
   * authentication failure. Mirrors the Python SDK's response so
   * connector-manager invalidates the acting user's credential and the web
   * layer surfaces the same reconnect CTA. Returns null when the failure is
   * not an auth failure or the acting source cannot be identified.
   */
  private mcpAuthRequiredResponse(
    message: string,
    source: Source | undefined,
    credentials: Record<string, unknown>
  ): Response | null {
    if (
      message !== MCP_AUTH_REQUIRED_MESSAGE &&
      !this.mcpAuthenticationError(message)
    ) {
      return null;
    }
    const sourceId =
      source?.id ??
      (typeof credentials.source_id === 'string' ? credentials.source_id : undefined);
    const sourceType = source?.source_type ?? this.sourceTypes[0];
    if (sourceId === undefined || sourceType === undefined) {
      return null;
    }
    return new Response(
      JSON.stringify({
        error: 'needs_user_auth',
        source_id: sourceId,
        source_type: sourceType,
        provider: this.oauthConfig?.provider ?? null,
        oauth_start_url: `/api/oauth/start?source_id=${sourceId}`,
      }),
      { status: 412, headers: { 'Content-Type': 'application/json' } }
    );
  }

  prepareMcpAuth(credentials: TCredentials): {
    env?: Record<string, string>;
    headers?: Record<string, string>;
  } {
    const server = this.mcpServer;
    if (server?.transport === 'http') {
      return { headers: this.prepareMcpHeaders(credentials) };
    }
    return { env: this.prepareMcpEnv(credentials) };
  }

  private async getAllActions(): Promise<ActionDefinition[]> {
    const manualActions = this.actions;
    const adapter = await this.getMcpAdapter();
    if (!adapter) {
      return manualActions;
    }
    const mcpActions = await adapter.getActionDefinitions();
    const inheritedActions = mcpActions.map((action) =>
      action.source_types.length === 0
        ? { ...action, source_types: this.sourceTypes }
        : action
    );
    const manualNames = new Set(manualActions.map((a) => a.name));
    return [
      ...manualActions,
      ...inheritedActions.filter((a) => !manualNames.has(a.name)),
    ];
  }

  async getManifest(connectorUrl: string): Promise<ConnectorManifest> {
    const adapter = await this.getMcpAdapter();
    const actions = await this.getAllActions();
    const manualActionNames = new Set(this.actions.map((action) => action.name));
    const mcpActions = adapter ? await adapter.getActionDefinitions() : [];
    const prompts = adapter ? await adapter.getPromptDefinitions() : [];
    const mcpActionNames = mcpActions
      .filter((action) => !manualActionNames.has(action.name))
      .map((action) => action.name);
    const skills = prompts.map((prompt) => ({
      id: `mcp:${prompt.name}`,
      title: prompt.name,
      description: prompt.description,
      mcp_prompt: prompt.name,
      source_types: this.sourceTypes,
    }));
    return {
      name: this.name,
      display_name: this.displayName,
      version: this.version,
      sync_modes: this.syncModes,
      connector_id: this.name,
      connector_url: connectorUrl,
      integration_type: 'connector',
      source_types: this.sourceTypes,
      description: this.description,
      actions,
      mcp_action_names: mcpActionNames,
      search_operators: this.searchOperators,
      extra_schema: this.extraSchema,
      attributes_schema: this.attributesSchema,
      read_only: false,
      mcp_enabled: adapter !== undefined,
      mcp_catalog_loaded: adapter?.hasCachedCatalog() ?? false,
      resources: adapter ? await adapter.getResourceDefinitions() : [],
      prompts,
      skills,
      oauth: this.oauthConfig,
    };
  }

  abstract sync(
    sourceConfig: TConfig,
    credentials: TCredentials,
    state: TState | null,
    ctx: SyncContext
  ): Promise<void>;

  cancel(_syncRunId: string): boolean {
    return false;
  }

  /**
   * Return env vars for a stdio MCP subprocess. Used only when
   * `mcpServer` returns a `StdioMcpServer`.
   *
   * @example
   * prepareMcpEnv(credentials) {
   *   return { GITHUB_PERSONAL_ACCESS_TOKEN: credentials.token };
   * }
   */
  prepareMcpEnv(_credentials: TCredentials): Record<string, string> {
    return {};
  }

  /**
   * Return HTTP headers for a remote MCP server. Used only when
   * `mcpServer` returns an `HttpMcpServer`.
   *
   * @example
   * prepareMcpHeaders(credentials) {
   *   return { Authorization: `Bearer ${credentials.token}` };
   * }
   */
  prepareMcpHeaders(_credentials: TCredentials): Record<string, string> {
    return {};
  }

  async executeAction(
    action: string,
    params: Record<string, unknown>,
    credentials: TCredentials,
    source?: Source,
    actor_email?: string
  ): Promise<Response> {
    const adapter = await this.getMcpAdapter();
    if (adapter) {
      try {
        const { env, headers } = this.prepareMcpAuth(credentials);
        const mcpActions = await adapter.getActionDefinitionsLive(env, headers);
        const mcpAction = mcpActions.find((definition) => definition.name === action);
        if (mcpAction) {
          const sourceReadOnly = source?.config?.read_only === true;
          if (sourceReadOnly && mcpAction.mode === 'write') {
            return ActionResponse.failure(
              `Action '${action}' is not allowed: source is read-only`
            ).toResponse(400);
          }
          const response = await adapter.executeTool(action, params, env, headers);
          if (response.status !== 'success' && response.error !== undefined) {
            // Terminal OAuth rejection: surface the standard 412 challenge
            // instead of a generic failure so connector-manager invalidates
            // the credential.
            const authResponse = this.mcpAuthRequiredResponse(
              response.error,
              source,
              credentials
            );
            if (authResponse) {
              return authResponse;
            }
          }
          return response.toResponse();
        }
      } catch (err) {
        const message = err instanceof Error ? err.message : String(err);
        // A live-validation failure caused by a terminal OAuth rejection must
        // produce the auth challenge; never fall through to a native action
        // (or a not-supported reply) for a tool that needs the user's
        // credential.
        const authResponse = this.mcpAuthRequiredResponse(message, source, credentials);
        if (authResponse) {
          return authResponse;
        }
        if (adapter.hasCachedAction(action)) {
          return ActionResponse.failure(
            `MCP action '${action}' could not be validated: ${message}`
          ).toResponse(400);
        }
        logger.warn({ err }, `MCP action lookup failed for ${action}`);
      }
    }
    return ActionResponse.notSupported(action).toResponse(404);
  }

  serve(options: ServeOptions = {}): void {
    const port = options.port ?? parseInt(process.env.PORT ?? '8000', 10);
    const host = options.host ?? '0.0.0.0';

    const app = createServer(this);
    const logger = getLogger(this.name);
    app.listen(port, host, () => {
      logger.info(`Connector ${this.name} v${this.version} listening on ${host}:${port}`);
    });
  }
}
