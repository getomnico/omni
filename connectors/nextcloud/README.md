# Nextcloud Connector

Syncs files and documents from a Nextcloud instance into Omni via WebDAV.

Enabled with the `nextcloud` Docker Compose profile. Configuration is managed through the Omni admin UI at `/admin/settings/integrations`.

## Features

- Full and incremental sync with ETag-based change detection
- Automatic deletion detection for removed files
- Adaptive listing: tries `Depth: infinity` first, falls back to breadth-first `Depth: 1` traversal
- Extension allow/deny lists and max file size filtering

## Authentication

Requires a Nextcloud **username** and **App Password** (Settings → Security → Devices & sessions).

## Source Configuration

| Field | Default | Description |
|---|---|---|
| `server_url` | *required* | Nextcloud server URL |
| `base_path` | `"/"` | Subdirectory to sync |
| `extension_allowlist` | `[]` (all) | Only sync these extensions |
| `extension_denylist` | `[]` | Skip these extensions |
| `max_file_size` | `0` (unlimited) | Max file size in bytes |
| `sync_enabled` | `true` | Enable periodic sync |

## Environment Variables

| Variable | Default | Description |
|---|---|---|
| `NEXTCLOUD_CONNECTOR_PORT` | `4014` | Port exposed by the connector |
| `RUST_LOG` | — | Log level (e.g. `debug`, `info`) |
