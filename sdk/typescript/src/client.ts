import { SdkClientError, ConfigurationError } from './errors.js';
import type { ConnectorEventPayload } from './models.js';
import { serializeConnectorEvent } from './models.js';

export class SdkClient {
  private readonly baseUrl: string;
  private readonly timeout: number;

  constructor(baseUrl?: string, timeout = 30000) {
    const url = baseUrl ?? process.env.CONNECTOR_MANAGER_URL;
    if (!url) {
      throw new ConfigurationError('CONNECTOR_MANAGER_URL environment variable not set');
    }
    this.baseUrl = url.replace(/\/$/, '');
    this.timeout = timeout;
  }

  static fromEnv(): SdkClient {
    return new SdkClient();
  }

  async emitEvent(
    syncRunId: string,
    sourceId: string,
    event: ConnectorEventPayload
  ): Promise<void> {
    const payload = {
      sync_run_id: syncRunId,
      source_id: sourceId,
      event: serializeConnectorEvent(event),
    };

    const response = await this.post('/sdk/events', payload);
    if (!response.ok) {
      const text = await response.text();
      throw new SdkClientError(
        `Failed to emit event: ${response.status} - ${text}`,
        response.status
      );
    }
  }

  async storeContent(
    syncRunId: string,
    content: string,
    contentType = 'text/plain'
  ): Promise<string> {
    const payload = {
      sync_run_id: syncRunId,
      content,
      content_type: contentType,
    };

    const response = await this.post('/sdk/content', payload);
    if (!response.ok) {
      const text = await response.text();
      throw new SdkClientError(
        `Failed to store content: ${response.status} - ${text}`,
        response.status
      );
    }

    const data = (await response.json()) as { content_id: string };
    return data.content_id;
  }

  async heartbeat(syncRunId: string): Promise<void> {
    const response = await this.post(`/sdk/sync/${syncRunId}/heartbeat`);
    if (!response.ok) {
      const text = await response.text();
      throw new SdkClientError(
        `Failed to heartbeat: ${response.status} - ${text}`,
        response.status
      );
    }
  }

  async incrementScanned(syncRunId: string): Promise<void> {
    const response = await this.post(`/sdk/sync/${syncRunId}/scanned`);
    if (!response.ok) {
      const text = await response.text();
      throw new SdkClientError(
        `Failed to increment scanned: ${response.status} - ${text}`,
        response.status
      );
    }
  }

  async complete(
    syncRunId: string,
    documentsScanned: number,
    documentsUpdated: number,
    newState?: Record<string, unknown>
  ): Promise<void> {
    const payload: Record<string, unknown> = {
      documents_scanned: documentsScanned,
      documents_updated: documentsUpdated,
    };
    if (newState !== undefined) {
      payload.new_state = newState;
    }

    const response = await this.post(`/sdk/sync/${syncRunId}/complete`, payload);
    if (!response.ok) {
      const text = await response.text();
      throw new SdkClientError(
        `Failed to complete: ${response.status} - ${text}`,
        response.status
      );
    }
  }

  async fail(syncRunId: string, error: string): Promise<void> {
    const response = await this.post(`/sdk/sync/${syncRunId}/fail`, { error });
    if (!response.ok) {
      const text = await response.text();
      throw new SdkClientError(
        `Failed to mark as failed: ${response.status} - ${text}`,
        response.status
      );
    }
  }

  async fetchSourceConfig(sourceId: string): Promise<{
    config: Record<string, unknown>;
    credentials: Record<string, unknown>;
    connector_state: Record<string, unknown> | null;
  }> {
    const response = await this.get(`/sdk/source/${sourceId}/sync-config`);
    if (!response.ok) {
      const text = await response.text();
      throw new SdkClientError(
        `Failed to fetch source config: ${response.status} - ${text}`,
        response.status
      );
    }
    return response.json() as Promise<{
      config: Record<string, unknown>;
      credentials: Record<string, unknown>;
      connector_state: Record<string, unknown> | null;
    }>;
  }

  private async get(path: string): Promise<Response> {
    const url = `${this.baseUrl}${path}`;
    return fetch(url, {
      method: 'GET',
      signal: AbortSignal.timeout(this.timeout),
    });
  }

  private async post(path: string, body?: unknown): Promise<Response> {
    const url = `${this.baseUrl}${path}`;
    const options: RequestInit = {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
      },
      signal: AbortSignal.timeout(this.timeout),
    };
    if (body !== undefined) {
      options.body = JSON.stringify(body);
    }
    return fetch(url, options);
  }
}
