import type { SdkClient } from './client.js';
import {
  EventType,
  type Document,
  type DocumentMetadata,
  type DocumentPermissions,
  type ConnectorEventPayload,
  type GroupMembershipEventPayload,
} from './models.js';
import { ContentStorage } from './storage.js';
import { getLogger } from './logger.js';

const logger = getLogger('sdk:context');

export class SyncContext {
  private readonly client: SdkClient;
  private readonly _syncRunId: string;
  private readonly _sourceId: string;
  private _state: Record<string, unknown>;
  private readonly abortController: AbortController;
  private _documentsEmitted = 0;
  private _documentsScanned = 0;
  private readonly _contentStorage: ContentStorage;

  constructor(
    client: SdkClient,
    syncRunId: string,
    sourceId: string,
    state?: Record<string, unknown>
  ) {
    this.client = client;
    this._syncRunId = syncRunId;
    this._sourceId = sourceId;
    this._state = state ?? {};
    this.abortController = new AbortController();
    this._contentStorage = new ContentStorage(client, syncRunId);
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

    await this.client.emitEvent(this._syncRunId, this._sourceId, event);
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

    await this.client.emitEvent(this._syncRunId, this._sourceId, event);
    this._documentsEmitted++;
  }

  async emitDeleted(externalId: string): Promise<void> {
    const event: ConnectorEventPayload = {
      type: EventType.DOCUMENT_DELETED,
      sync_run_id: this._syncRunId,
      source_id: this._sourceId,
      document_id: externalId,
    };

    await this.client.emitEvent(this._syncRunId, this._sourceId, event);
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

    await this.client.emitEvent(this._syncRunId, this._sourceId, event);
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
   * Persists `state` to the manager so an interrupted sync can resume from
   * this point. Callers MUST `await` every `emit()` for documents covered
   * by `state` before calling this — otherwise a resume could skip events
   * that hadn't yet been enqueued.
   */
  async saveState(state: Record<string, unknown>): Promise<void> {
    this._state = state;
    await this.client.updateConnectorState(this._sourceId, state);
    await this.client.heartbeat(this._syncRunId);
  }

  async complete(newState?: Record<string, unknown>): Promise<void> {
    await this.client.complete(
      this._syncRunId,
      this._documentsScanned,
      this._documentsEmitted,
      newState
    );
  }

  async fail(error: string): Promise<void> {
    await this.client.fail(this._syncRunId, error);
  }

  isCancelled(): boolean {
    return this.abortController.signal.aborted;
  }

  _setCancelled(): void {
    this.abortController.abort();
  }
}
