import type { SyncContext } from './context.js';
import type { McpAdapter, McpServer } from './mcp-adapter.js';
import type {
  ConnectorManifest,
  ActionDefinition,
  SearchOperator,
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

  /**
   * Discover MCP tools/resources/prompts and cache them. Called when
   * credentials first become available (e.g., during initial sync).
   */
  async bootstrapMcp(credentials: TCredentials): Promise<void> {
    const adapter = await this.getMcpAdapter();
    if (!adapter) {
      return;
    }
    const { env, headers } = this.prepareMcpAuth(credentials);
    logger.info('Bootstrapping MCP: discovering tools');
    try {
      await adapter.discover(env, headers);
    } catch (err) {
      logger.warn({ err }, 'MCP bootstrap failed');
    }
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
    const manualNames = new Set(manualActions.map((a) => a.name));
    return [...manualActions, ...mcpActions.filter((a) => !manualNames.has(a.name))];
  }

  async getManifest(connectorUrl: string): Promise<ConnectorManifest> {
    const adapter = await this.getMcpAdapter();
    return {
      name: this.name,
      display_name: this.displayName,
      version: this.version,
      sync_modes: this.syncModes,
      connector_id: this.name,
      connector_url: connectorUrl,
      source_types: this.sourceTypes,
      description: this.description,
      actions: await this.getAllActions(),
      search_operators: this.searchOperators,
      extra_schema: this.extraSchema,
      attributes_schema: this.attributesSchema,
      mcp_enabled: adapter !== undefined,
      resources: adapter ? await adapter.getResourceDefinitions() : [],
      prompts: adapter ? await adapter.getPromptDefinitions() : [],
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
    credentials: TCredentials
  ): Promise<Response> {
    const adapter = await this.getMcpAdapter();
    if (adapter) {
      const { env, headers } = this.prepareMcpAuth(credentials);
      const mcpActions = await adapter.getActionDefinitions(env, headers);
      const mcpToolNames = new Set(mcpActions.map((a) => a.name));
      if (mcpToolNames.has(action)) {
        const response = await adapter.executeTool(action, params, env, headers);
        return response.toResponse();
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
