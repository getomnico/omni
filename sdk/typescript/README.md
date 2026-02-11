# @getomnico/connector

TypeScript SDK for building Omni connectors.

## Installation

```bash
npm install @getomnico/connector
```

## Quick Start

```typescript
import {
  Connector,
  SyncContext,
  type Document,
} from '@getomnico/connector';

class MyConnector extends Connector {
  name = 'my-connector';
  version = '1.0.0';
  syncModes = ['full', 'incremental'];

  async sync(
    sourceConfig: Record<string, unknown>,
    credentials: Record<string, unknown>,
    state: Record<string, unknown> | null,
    ctx: SyncContext
  ): Promise<void> {
    const cursor = state?.cursor as string | undefined;

    for (const item of await fetchItems(credentials, cursor)) {
      if (ctx.isCancelled()) {
        await ctx.fail('Cancelled by user');
        return;
      }

      const contentId = await ctx.contentStorage.save(item.content);

      await ctx.emit({
        external_id: item.id,
        title: item.title,
        content_id: contentId,
        metadata: { url: item.url },
      });

      await ctx.incrementScanned();
    }

    await ctx.complete({ cursor: latestCursor });
  }
}

new MyConnector().serve({ port: 8000 });
```

## Environment Variables

- `CONNECTOR_MANAGER_URL` - Required. URL of the connector-manager service
- `PORT` - HTTP server port (default: 8000)

## API Reference

### Connector

Abstract base class for building connectors.

```typescript
abstract class Connector {
  abstract name: string;
  abstract version: string;
  syncModes: string[] = ['full'];
  actions: ActionDefinition[] = [];

  abstract sync(
    sourceConfig: Record<string, unknown>,
    credentials: Record<string, unknown>,
    state: Record<string, unknown> | null,
    ctx: SyncContext
  ): Promise<void>;

  cancel(syncRunId: string): boolean;
  executeAction(action: string, params: Record<string, unknown>, credentials: Record<string, unknown>): Promise<ActionResponse>;
  serve(options?: { port?: number; host?: string }): void;
}
```

### SyncContext

Context object passed to the `sync` method.

```typescript
class SyncContext {
  syncRunId: string;
  sourceId: string;
  state: Record<string, unknown>;
  contentStorage: ContentStorage;
  documentsEmitted: number;
  documentsScanned: number;

  emit(doc: Document): Promise<void>;
  emitUpdated(doc: Document): Promise<void>;
  emitDeleted(externalId: string): Promise<void>;
  emitError(externalId: string, error: string): void;
  incrementScanned(): Promise<void>;
  saveState(state: Record<string, unknown>): Promise<void>;
  complete(newState?: Record<string, unknown>): Promise<void>;
  fail(error: string): Promise<void>;
  isCancelled(): boolean;
}
```

### ContentStorage

Helper for storing document content.

```typescript
class ContentStorage {
  save(content: string, contentType?: string): Promise<string>;
  saveBinary(content: Buffer, contentType?: string): Promise<string>;
}
```

### Document

Document model for emitting to Omni.

```typescript
interface Document {
  external_id: string;
  title: string;
  content_id: string;
  metadata?: DocumentMetadata;
  permissions?: DocumentPermissions;
  attributes?: Record<string, unknown>;
}

interface DocumentMetadata {
  title?: string;
  author?: string;
  created_at?: string;
  updated_at?: string;
  mime_type?: string;
  size?: string;
  url?: string;
  path?: string;
  extra?: Record<string, unknown>;
}

interface DocumentPermissions {
  public?: boolean;
  users?: string[];
  groups?: string[];
}
```

## HTTP Endpoints

The connector server exposes these endpoints:

- `GET /health` - Health check
- `GET /manifest` - Returns connector manifest
- `POST /sync` - Triggers a sync operation
- `POST /cancel` - Cancels a running sync
- `POST /action` - Executes a connector action

## Examples

See the [examples](./examples) directory for complete examples:

- [RSS Connector](./examples/rss-connector.ts) - Syncs articles from RSS feeds

## Development

```bash
# Install dependencies
npm install

# Build
npm run build

# Run tests
npm test
```

## License

MIT
