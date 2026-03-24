import type { McpServer } from '@modelcontextprotocol/sdk/server/mcp.js';

import type { SyncContext } from './context.js';
import type {
  ConnectorManifest,
  ActionDefinition,
  ActionResponse,
  SearchOperator,
} from './models.js';
import { createActionResponseNotSupported } from './models.js';
import { McpAdapter } from './mcp-adapter.js';
import { createServer } from './server.js';
import { getLogger } from './logger.js';

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

  private _mcpAdapter: McpAdapter | null = null;

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
   */
  get mcpServer(): McpServer | undefined {
    return undefined;
  }

  get mcpAdapter(): McpAdapter | undefined {
    if (this._mcpAdapter !== null) {
      return this._mcpAdapter;
    }
    const server = this.mcpServer;
    if (!server) {
      return undefined;
    }
    this._mcpAdapter = new McpAdapter(server);
    return this._mcpAdapter;
  }

  private async getAllActions(): Promise<ActionDefinition[]> {
    const manualActions = this.actions;
    const adapter = this.mcpAdapter;
    if (!adapter) {
      return manualActions;
    }
    const mcpActions = await adapter.getActionDefinitions();
    const manualNames = new Set(manualActions.map((a) => a.name));
    return [...manualActions, ...mcpActions.filter((a) => !manualNames.has(a.name))];
  }

  async getManifest(connectorUrl: string): Promise<ConnectorManifest> {
    const adapter = this.mcpAdapter;
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
  ): Promise<ActionResponse> {
    const adapter = this.mcpAdapter;
    if (adapter) {
      const mcpActions = await adapter.getActionDefinitions();
      const mcpToolNames = new Set(mcpActions.map((a) => a.name));
      if (mcpToolNames.has(action)) {
        this.prepareMcpEnv(credentials);
        return adapter.executeTool(action, params);
      }
    }
    return createActionResponseNotSupported(action);
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
