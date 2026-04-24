import type { SyncContext } from './context.js';
import type {
  ConnectorManifest,
  ActionDefinition,
  SearchOperator,
} from './models.js';
import { ActionResponse } from './models.js';
import { createServer } from './server.js';
import { getLogger } from './logger.js';

export interface McpAdapter {
  getActionDefinitions(): Promise<ActionDefinition[]>;
  getResourceDefinitions(): Promise<unknown[]>;
  getPromptDefinitions(): Promise<unknown[]>;
  executeTool(
    name: string,
    params: Record<string, unknown>,
  ): Promise<ActionResponse>;
  readResource(uri: string): Promise<unknown>;
  getPrompt(name: string, args?: Record<string, string>): Promise<unknown>;
}

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
   * Return an MCP McpServer instance if this connector supports MCP.
   * Override this getter to enable MCP support.
   * Requires @modelcontextprotocol/sdk as a dependency.
   */
  get mcpServer(): unknown | undefined {
    return undefined;
  }

  async getMcpAdapter(): Promise<McpAdapter | undefined> {
    if (this._mcpAdapter !== null) {
      return this._mcpAdapter as ReturnType<typeof this.getMcpAdapter> extends Promise<infer T> ? T : never;
    }
    const server = this.mcpServer;
    if (!server) {
      return undefined;
    }
    const { McpAdapter } = await import('./mcp-adapter.js');
    this._mcpAdapter = new McpAdapter(server as any);
    return this._mcpAdapter as any;
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
      resources: adapter ? await adapter.getResourceDefinitions() as any[] : [],
      prompts: adapter ? await adapter.getPromptDefinitions() as any[] : [],
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
   * Set up environment for MCP tool/resource/prompt calls.
   * Override to bridge Omni credentials to the env vars your MCP server expects.
   */
  prepareMcpEnv(_credentials: TCredentials): void {
    // no-op by default
  }

  async executeAction(
    action: string,
    params: Record<string, unknown>,
    credentials: TCredentials
  ): Promise<Response> {
    const adapter = await this.getMcpAdapter();
    if (adapter) {
      const mcpActions = await adapter.getActionDefinitions();
      const mcpToolNames = new Set(mcpActions.map((a) => a.name));
      if (mcpToolNames.has(action)) {
        this.prepareMcpEnv(credentials);
        const response = await adapter.executeTool(action, params);
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
