import type { SdkClient } from './client.js';
import {
  EventType,
  SyncMode,
  type Document,
  type DocumentMetadata,
  type DocumentPermissions,
  type ConnectorEventPayload,
  type GroupMembershipEventPayload,
} from './models.js';
import { ContentStorage } from './storage.js';
import { getLogger } from './logger.js';

const logger = getLogger('sdk:context');

/** Buffer thresholds (size, timeMs) per sync mode. `null` timeMs = flush-on-emit. */
function thresholdsFor(syncMode: SyncMode): { size: number; timeMs: number | null } {
  if (syncMode === SyncMode.FULL) return { size: 500, timeMs: 300_000 };
  if (syncMode === SyncMode.REALTIME) return { size: 1, timeMs: null };
  return { size: 100, timeMs: 60_000 }; // Incremental (default)
}

export class SyncContext {
  private readonly client: SdkClient;
  private readonly _syncRunId: string;
  private readonly _sourceId: string;
  private _state: Record<string, unknown>;
  private readonly abortController: AbortController;
  private _documentsEmitted = 0;
  private _documentsScanned = 0;
  private readonly _contentStorage: ContentStorage;
  private readonly _syncMode: SyncMode;
  private readonly bufferSizeThreshold: number;
  private readonly bufferTimeThresholdMs: number | null;
  private eventBuffer: ConnectorEventPayload[] = [];
  private oldestEventAt: number | null = null;

  constructor(
    client: SdkClient,
    syncRunId: string,
    sourceId: string,
    state?: Record<string, unknown>,
    syncMode: SyncMode = SyncMode.INCREMENTAL
  ) {
    this.client = client;
    this._syncRunId = syncRunId;
    this._sourceId = sourceId;
    this._state = state ?? {};
    this.abortController = new AbortController();
    this._contentStorage = new ContentStorage(client, syncRunId);
    this._syncMode = syncMode;
    const thresholds = thresholdsFor(syncMode);
    this.bufferSizeThreshold = thresholds.size;
    this.bufferTimeThresholdMs = thresholds.timeMs;
  }

  get syncRunId(): string {
    return this._syncRunId;
  }

  get sourceId(): string {
    return this._sourceId;
  }

  get state(): Record<string, unknown> {
    return this._state;
  }

  get contentStorage(): ContentStorage {
    return this._contentStorage;
  }

  get documentsEmitted(): number {
    return this._documentsEmitted;
  }

  get documentsScanned(): number {
    return this._documentsScanned;
  }

  private async bufferEvent(event: ConnectorEventPayload): Promise<void> {
    this.eventBuffer.push(event);
    if (this.oldestEventAt === null) {
      this.oldestEventAt = Date.now();
    }

    const sizeHit = this.eventBuffer.length >= this.bufferSizeThreshold;
    const timeHit =
      this.bufferTimeThresholdMs !== null &&
      this.oldestEventAt !== null &&
      Date.now() - this.oldestEventAt >= this.bufferTimeThresholdMs;
    if (sizeHit || timeHit) {
      await this.flush();
    }
  }

  async flush(): Promise<void> {
    if (this.eventBuffer.length === 0) {
      return;
    }
    const batch = this.eventBuffer;
    this.eventBuffer = [];
    this.oldestEventAt = null;
    await this.client.emitEventBatch(this._syncRunId, this._sourceId, batch);
  }

  async emit(doc: Document): Promise<void> {
    const event: ConnectorEventPayload = {
      type: EventType.DOCUMENT_CREATED,
      sync_run_id: this._syncRunId,
      source_id: this._sourceId,
      document_id: doc.external_id,
      content_id: doc.content_id,
      metadata: doc.metadata,
      permissions: doc.permissions,
      attributes: doc.attributes,
    };
    await this.bufferEvent(event);
    this._documentsEmitted++;
  }

  async emitUpdated(doc: Document): Promise<void> {
    const event: ConnectorEventPayload = {
      type: EventType.DOCUMENT_UPDATED,
      sync_run_id: this._syncRunId,
      source_id: this._sourceId,
      document_id: doc.external_id,
      content_id: doc.content_id,
      metadata: doc.metadata,
      permissions: doc.permissions,
      attributes: doc.attributes,
    };
    await this.bufferEvent(event);
    this._documentsEmitted++;
  }

  async emitDeleted(externalId: string): Promise<void> {
    const event: ConnectorEventPayload = {
      type: EventType.DOCUMENT_DELETED,
      sync_run_id: this._syncRunId,
      source_id: this._sourceId,
      document_id: externalId,
    };
    await this.bufferEvent(event);
  }

  async emitGroupMembership(
    groupEmail: string,
    memberEmails: string[],
    groupName?: string,
  ): Promise<void> {
    const event: GroupMembershipEventPayload = {
      type: EventType.GROUP_MEMBERSHIP_SYNC,
      sync_run_id: this._syncRunId,
      source_id: this._sourceId,
      group_email: groupEmail,
      group_name: groupName,
      member_emails: memberEmails,
    };
    await this.bufferEvent(event);
  }

  emitError(externalId: string, error: string): void {
    logger.warn(`Document error for ${externalId}: ${error}`);
  }

  async incrementScanned(): Promise<void> {
    this._documentsScanned++;
    await this.client.incrementScanned(this._syncRunId);
  }

  /**
   * Checkpoint state for resumability. Call periodically for long syncs.
   *
   * Flushes buffered events first — without this, a crash right after
   * checkpointing would lose events that the connector considered emitted
   * (the next run resumes past them).
   */
  async saveState(state: Record<string, unknown>): Promise<void> {
    await this.flush();
    this._state = state;
    await this.client.updateConnectorState(this._sourceId, state);
    await this.client.heartbeat(this._syncRunId);
  }

  async complete(newState?: Record<string, unknown>): Promise<void> {
    await this.flush();
    await this.client.complete(
      this._syncRunId,
      this._documentsScanned,
      this._documentsEmitted,
      newState
    );
  }

  async fail(error: string): Promise<void> {
    try {
      await this.flush();
    } catch (e) {
      logger.warn(
        `flush before fail() failed (continuing): sync_run=${this._syncRunId}: ${e}`
      );
    }
    await this.client.fail(this._syncRunId, error);
  }

  isCancelled(): boolean {
    return this.abortController.signal.aborted;
  }

  _setCancelled(): void {
    this.abortController.abort();
  }
}
