import express, { type Express, type Request, type Response } from 'express';

import { SdkClient } from './client.js';
import type { Connector } from './connector.js';
import { SyncContext } from './context.js';
import {
  SyncMode,
  SyncRequestSchema,
  CancelRequestSchema,
  ActionRequestSchema,
  ResourceRequestSchema,
  PromptRequestSchema,
  createSyncResponseStarted,
  createSyncResponseError,
  createActionResponseFailure,
} from './models.js';
import { getLogger } from './logger.js';

const logger = getLogger('sdk:server');

const REGISTRATION_INTERVAL_MS = 30_000;

function buildConnectorUrl(): string {
  const hostname = process.env.CONNECTOR_HOST_NAME;
  if (!hostname) {
    throw new Error(
      'CONNECTOR_HOST_NAME environment variable is required. ' +
      'Set it to this connector\'s hostname (e.g. the Docker service name).'
    );
  }
  const port = process.env.PORT;
  if (!port) {
    throw new Error('PORT environment variable is required.');
  }
  return `http://${hostname}:${port}`;
}

export function createServer(connector: Connector): Express {
  const app = express();
  app.use(express.json());

  const activeSyncs = new Map<string, SyncContext>();
  let sdkClient: SdkClient | null = null;

  function getSdkClient(): SdkClient {
    if (sdkClient === null) {
      sdkClient = SdkClient.fromEnv();
    }
    return sdkClient;
  }

  // Start registration loop
  const connectorUrl = buildConnectorUrl();
  const registerOnce = async () => {
    try {
      const manifest = await connector.getManifest(connectorUrl);
      await getSdkClient().register(manifest as unknown as Record<string, unknown>);
      logger.info('Registered with connector manager');
    } catch (err) {
      logger.warn({ err }, 'Registration failed');
    }
  };

  registerOnce();
  setInterval(registerOnce, REGISTRATION_INTERVAL_MS);

  app.get('/health', (_req: Request, res: Response) => {
    res.json({ status: 'healthy', service: connector.name });
  });

  app.get('/manifest', async (_req: Request, res: Response) => {
    const manifest = await connector.getManifest(connectorUrl);
    res.json(manifest);
  });

  app.get('/sync/:syncRunId', (req: Request, res: Response) => {
    const { syncRunId } = req.params;
    let running = false;
    for (const ctx of activeSyncs.values()) {
      if (ctx.syncRunId === syncRunId) {
        running = true;
        break;
      }
    }
    res.json({ running });
  });

  app.post('/sync', async (req: Request, res: Response) => {
    const parseResult = SyncRequestSchema.safeParse(req.body);
    if (!parseResult.success) {
      res.status(400).json(createSyncResponseError('Invalid request body'));
      return;
    }

    const {
      sync_run_id: syncRunId,
      source_id: sourceId,
      sync_mode: syncModeStr,
    } = parseResult.data;

    const isValidSyncMode = (Object.values(SyncMode) as string[]).includes(syncModeStr);
    if (!isValidSyncMode) {
      logger.warn(
        `Unknown sync_mode '${syncModeStr}'; defaulting to Incremental batching`
      );
    }
    const syncMode: SyncMode = isValidSyncMode
      ? (syncModeStr as SyncMode)
      : SyncMode.INCREMENTAL;

    logger.info(`Sync triggered for source ${sourceId} (sync_run_id: ${syncRunId})`);

    if (activeSyncs.has(sourceId)) {
      res.status(409).json(
        createSyncResponseError('Sync already in progress for this source')
      );
      return;
    }

    let sourceData: {
      config: Record<string, unknown>;
      credentials: Record<string, unknown>;
      state: Record<string, unknown> | null;
    };
    try {
      const data = await getSdkClient().fetchSourceConfig(sourceId);
      sourceData = {
        config: data.config,
        credentials: data.credentials,
        state: data.connector_state,
      };
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      if (message.includes('404')) {
        res.status(404).json(createSyncResponseError(`Source not found: ${sourceId}`));
      } else {
        logger.error({ err: error }, 'Failed to fetch source data');
        res.status(500).json(
          createSyncResponseError(`Failed to fetch source data: ${message}`)
        );
      }
      return;
    }

    const ctx = new SyncContext(
      getSdkClient(),
      syncRunId,
      sourceId,
      sourceData.state ?? undefined,
      syncMode
    );
    activeSyncs.set(sourceId, ctx);

    const runSync = async (): Promise<void> => {
      try {
        await connector.sync(
          sourceData.config,
          sourceData.credentials,
          sourceData.state,
          ctx
        );
      } catch (error) {
        const message = error instanceof Error ? error.message : String(error);
        logger.error({ err: error }, `Sync ${syncRunId} failed`);
        try {
          await ctx.fail(message);
        } catch (failError) {
          logger.error({ err: failError }, 'Failed to report sync failure');
        }
      } finally {
        activeSyncs.delete(sourceId);
      }
    };

    runSync();

    res.status(200).json(createSyncResponseStarted());
  });

  app.post('/cancel', (req: Request, res: Response) => {
    const parseResult = CancelRequestSchema.safeParse(req.body);
    if (!parseResult.success) {
      res.status(400).json({ status: 'error', message: 'Invalid request body' });
      return;
    }

    const { sync_run_id: syncRunId } = parseResult.data;
    logger.info(`Cancel requested for sync ${syncRunId}`);

    for (const [sourceId, ctx] of activeSyncs.entries()) {
      if (ctx.syncRunId === syncRunId) {
        ctx._setCancelled();
        connector.cancel(syncRunId);
        res.json({ status: 'cancelled' });
        return;
      }
    }

    res.json({ status: 'not_found' });
  });

  app.post('/action', async (req: Request, res: Response) => {
    const parseResult = ActionRequestSchema.safeParse(req.body);
    if (!parseResult.success) {
      res.status(400).json(createActionResponseFailure('Invalid request body'));
      return;
    }

    const { action, params, credentials } = parseResult.data;
    logger.info(`Action requested: ${action}`);

    try {
      const response = await connector.executeAction(action, params, credentials);
      res.json(response);
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      logger.error({ err: error }, `Action ${action} failed`);
      res.json(createActionResponseFailure(message));
    }
  });

  app.post('/resource', async (req: Request, res: Response) => {
    const adapter = await connector.getMcpAdapter();
    if (!adapter) {
      res.status(404).json({ error: 'MCP not enabled for this connector' });
      return;
    }

    const parseResult = ResourceRequestSchema.safeParse(req.body);
    if (!parseResult.success) {
      res.status(400).json({ error: 'Invalid request body' });
      return;
    }

    const { uri, credentials } = parseResult.data;
    logger.info(`Resource requested: ${uri}`);

    try {
      connector.prepareMcpEnv(credentials as any);
      const result = await adapter.readResource(uri);
      res.json(result);
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      logger.error({ err }, `Resource read failed for ${uri}`);
      res.status(500).json({ error: message });
    }
  });

  app.post('/prompt', async (req: Request, res: Response) => {
    const adapter = await connector.getMcpAdapter();
    if (!adapter) {
      res.status(404).json({ error: 'MCP not enabled for this connector' });
      return;
    }

    const parseResult = PromptRequestSchema.safeParse(req.body);
    if (!parseResult.success) {
      res.status(400).json({ error: 'Invalid request body' });
      return;
    }

    const { name, arguments: args, credentials } = parseResult.data;
    logger.info(`Prompt requested: ${name}`);

    try {
      connector.prepareMcpEnv(credentials as any);
      const result = await adapter.getPrompt(name, args as Record<string, string> | undefined);
      res.json(result);
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      logger.error({ err }, `Prompt get failed for ${name}`);
      res.status(500).json({ error: message });
    }
  });

  return app;
}
