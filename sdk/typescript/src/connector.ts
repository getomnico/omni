import type { SyncContext } from './context.js';
import type {
  ConnectorManifest,
  ActionDefinition,
  ActionResponse,
  SearchOperator,
} from './models.js';
import { createActionResponseNotSupported } from './models.js';
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

  getManifest(connectorUrl?: string): ConnectorManifest {
    return {
      name: this.name,
      display_name: this.displayName,
      version: this.version,
      sync_modes: this.syncModes,
      connector_id: this.name,
      connector_url: connectorUrl,
      source_types: this.sourceTypes,
      description: this.description,
      actions: this.actions,
      search_operators: this.searchOperators,
      extra_schema: this.extraSchema,
      attributes_schema: this.attributesSchema,
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

  executeAction(
    action: string,
    _params: Record<string, unknown>,
    _credentials: TCredentials
  ): Promise<ActionResponse> {
    return Promise.resolve(createActionResponseNotSupported(action));
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
