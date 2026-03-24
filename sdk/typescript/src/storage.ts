import type { SdkClient } from './client.js';

export class ContentStorage {
  private readonly client: SdkClient;
  private readonly syncRunId: string;

  constructor(client: SdkClient, syncRunId: string) {
    this.client = client;
    this.syncRunId = syncRunId;
  }

  async save(content: string, contentType = 'text/plain'): Promise<string> {
    return this.client.storeContent(this.syncRunId, content, contentType);
  }

  async extractAndStore(
    data: Buffer | Uint8Array,
    mimeType: string,
    filename?: string
  ): Promise<string> {
    return this.client.extractAndStore(this.syncRunId, data, mimeType, filename);
  }

  async saveBinary(
    content: Buffer,
    contentType = 'application/octet-stream'
  ): Promise<string> {
    const encoded = content.toString('base64');
    return this.client.storeContent(this.syncRunId, encoded, contentType);
  }
}
