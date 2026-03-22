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

export abstract class Connector {
  abstract readonly name: string;
  abstract readonly version: string;

  get displayName(): string {
    return this.name;
  }

  readonly syncModes: string[] = ['full'];
  readonly actions: ActionDefinition[] = [];
  readonly searchOperators: SearchOperator[] = [];

  getManifest(): ConnectorManifest {
    return {
      name: this.name,
      display_name: this.displayName,
      version: this.version,
      sync_modes: this.syncModes,
      actions: this.actions,
      search_operators: this.searchOperators,
    };
  }

  abstract sync(
    sourceConfig: Record<string, unknown>,
    credentials: Record<string, unknown>,
    state: Record<string, unknown> | null,
    ctx: SyncContext
  ): Promise<void>;

  cancel(_syncRunId: string): boolean {
    return false;
  }

  executeAction(
    action: string,
    _params: Record<string, unknown>,
    _credentials: Record<string, unknown>
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
