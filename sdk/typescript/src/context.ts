import type { SdkClient } from './client.js';
import {
  EventType,
  SyncMode,
  UserFilterMode,
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
  private readonly _sourceType: string | null;
  private readonly _userFilterMode: UserFilterMode;
  private readonly _userWhitelist: ReadonlySet<string>;
  private readonly _userBlacklist: ReadonlySet<string>;

  constructor(
    client: SdkClient,
    syncRunId: string,
    sourceId: string,
    state?: Record<string, unknown>,
    syncMode: SyncMode = SyncMode.INCREMENTAL,
    documentsScanned = 0,
    documentsUpdated = 0,
    options?: {
      sourceType?: string | null;
      userFilterMode?: UserFilterMode;
      userWhitelist?: string[] | null;
      userBlacklist?: string[] | null;
    }
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
    // Seed counters from the dispatch payload so resume continues from
    // the manager's running tally rather than restarting at zero.
    this._documentsScanned = documentsScanned;
    this._documentsEmitted = documentsUpdated;
    this._sourceType = options?.sourceType ?? null;
    this._userFilterMode = options?.userFilterMode ?? UserFilterMode.ALL;
    // Store as lowercased set so shouldIndexUser is case-insensitive
    // (mirrors Python's should_index_user; deliberately differs from
    // Rust's case-sensitive Source::should_index_user).
    this._userWhitelist = new Set(
      (options?.userWhitelist ?? []).map((e) => e.toLowerCase())
    );
    this._userBlacklist = new Set(
      (options?.userBlacklist ?? []).map((e) => e.toLowerCase())
    );
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

  /**
   * The sync mode the manager dispatched this run with — full, incremental,
   * or realtime. Connectors should branch reset/resume behavior on this,
   * not on `state` presence: a manual "Full" trigger from the UI carries
   * existing state but should still reset cursors and run delete
   * reconciliation.
   */
  get syncMode(): SyncMode {
    return this._syncMode;
  }
  
  get sourceType(): string | null {
    return this._sourceType;
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
    // Shim Document.title into metadata.title — the indexer reads only
    // metadata.title and falls back to "Untitled" otherwise. Non-mutating
    // spread so the caller's Document is never modified (broader than
    // Python's conditional shim, which skips bare-metadata docs).
    const metadata: DocumentMetadata = { ...(doc.metadata ?? {}) };
    if (!metadata.title) {
      metadata.title = doc.title;
    }
    const event: ConnectorEventPayload = {
      type: EventType.DOCUMENT_CREATED,
      sync_run_id: this._syncRunId,
      source_id: this._sourceId,
      document_id: doc.external_id,
      content_id: doc.content_id,
      metadata,
      permissions: doc.permissions,
      attributes: doc.attributes,
    };
    await this.bufferEvent(event);
    this._documentsEmitted++;
  }

  async emitUpdated(doc: Document): Promise<void> {
    const metadata: DocumentMetadata = { ...(doc.metadata ?? {}) };
    if (!metadata.title) {
      metadata.title = doc.title;
    }
    const event: ConnectorEventPayload = {
      type: EventType.DOCUMENT_UPDATED,
      sync_run_id: this._syncRunId,
      source_id: this._sourceId,
      document_id: doc.external_id,
      content_id: doc.content_id,
      metadata,
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
   * Bump the server-side documents_updated counter for this sync run.
   *
   * Mirrors Rust SDK's increment_updated. The connector author chooses
   * what counts as "updated" — typically called once per successfully
   * persisted emit/emitUpdated, or batched (e.g. once per processed
   * chunk). Errors propagate so the caller can decide whether a stat
   * miss should fail the sync or be swallowed.
   */
  async incrementUpdated(count: number): Promise<void> {
    if (count <= 0) return;
    await this.client.incrementUpdated(this._syncRunId, count);
  }

  /**
   * Check whether a user should be indexed under this source's user-filter
   * settings. Mirrors the Python SDK's should_index_user.
   *
   * Connectors call this themselves before emitting per-user records — the
   * SDK does NOT auto-skip on emit, since the connector knows whether a
   * given doc is "owned by a user" or workspace-shared.
   *
   * Empty email: ALL admits, WHITELIST/BLACKLIST reject (matches Python).
   * Comparison is case-insensitive (Rust's equivalent is case-sensitive —
   * deliberate divergence; lowercasing here is the friendlier default and
   * matches operator expectations).
   */
  shouldIndexUser(userEmail: string): boolean {
    if (this._userFilterMode === UserFilterMode.ALL) return true;
    const email = userEmail.toLowerCase();
    if (!email) return false;
    if (this._userFilterMode === UserFilterMode.WHITELIST) {
      return this._userWhitelist.has(email);
    }
    if (this._userFilterMode === UserFilterMode.BLACKLIST) {
      return !this._userBlacklist.has(email);
    }
    return true;
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
    // sdk_complete is body-less since 7c21fd10 — newState in the /complete
    // body is silently dropped server-side. Persist via the connector-state
    // endpoint instead (same path saveState() takes).
    if (newState !== undefined) {
      this._state = newState;
      await this.client.updateConnectorState(this._sourceId, newState);
    }
    await this.client.complete(this._syncRunId);
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
