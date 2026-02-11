# Omni Connector SDK for Python

Python SDK for building custom connectors for [Omni](https://github.com/getomnico/omni), an open-source enterprise search platform.

## Installation

```bash
pip install omni-connector
```

## Quick Start

Create a custom connector by inheriting from the `Connector` base class:

```python
from omni_connector import (
    Connector,
    Document,
    DocumentMetadata,
    DocumentPermissions,
    SyncContext,
)

class MyConnector(Connector):
    @property
    def name(self) -> str:
        return "my-connector"

    @property
    def version(self) -> str:
        return "1.0.0"

    @property
    def sync_modes(self) -> list[str]:
        return ["full", "incremental"]

    async def sync(
        self,
        source_config: dict,
        credentials: dict,
        state: dict | None,
        ctx: SyncContext,
    ) -> None:
        cursor = state.get("cursor") if state else None

        async for item in fetch_items(credentials, cursor):
            if ctx.is_cancelled():
                await ctx.fail("Cancelled by user")
                return

            # Store content
            content_id = await ctx.content_storage.save(item["content"])

            # Emit document
            await ctx.emit(Document(
                external_id=item["id"],
                title=item["title"],
                content_id=content_id,
                metadata=DocumentMetadata(url=item["url"]),
                permissions=DocumentPermissions(public=True),
            ))

            await ctx.increment_scanned()
            cursor = item["cursor"]

        await ctx.complete(new_state={"cursor": cursor})

if __name__ == "__main__":
    MyConnector().serve(port=8000)
```

## Environment Variables

- `CONNECTOR_MANAGER_URL` - Required. URL of the connector-manager service
- `PORT` - HTTP server port (default: 8000)

## SyncContext API

The `SyncContext` object is passed to your `sync()` method and provides:

- `ctx.emit(doc)` - Emit a new document
- `ctx.emit_updated(doc)` - Emit a document update
- `ctx.emit_deleted(external_id)` - Mark a document as deleted
- `ctx.increment_scanned()` - Increment the scanned counter
- `ctx.save_state(state)` - Checkpoint state for resumability
- `ctx.complete(new_state)` - Mark sync as completed
- `ctx.fail(error)` - Mark sync as failed
- `ctx.is_cancelled()` - Check if sync was cancelled
- `ctx.content_storage.save(content)` - Store content and get content_id

## Connector Protocol (HTTP)

The SDK automatically exposes these HTTP endpoints:

- `GET /health` - Health check
- `GET /manifest` - Connector capabilities
- `POST /sync` - Trigger a sync
- `POST /cancel` - Cancel a running sync
- `POST /action` - Execute an action

## Examples

See the `examples/` directory for complete examples:

- `rss_connector.py` - RSS feed connector

## Development

```bash
# Install with dev dependencies
pip install -e ".[dev]"

# Run tests
pytest

# Type check
mypy omni_connector

# Lint
ruff check omni_connector
```

## License

MIT
