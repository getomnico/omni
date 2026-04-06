# Paperless-ngx Connector for Omni

Syncs documents from a [paperless-ngx](https://docs.paperless-ngx.com/) instance into Omni via its REST API. **Read-only** — the connector never modifies data in paperless-ngx.

## What Gets Synced

Each paperless-ngx document is ingested with:

- **OCR content** — full text extracted by paperless-ngx
- **Classification** — correspondent, document type, and tags
- **Storage path** and **archive serial number** (ASN)
- **Custom fields** — resolved to human-readable field names
- **Notes** — user annotations with author and timestamp
- **Dates** — created, added, and modified timestamps

## Sync Modes

| Mode | Behaviour |
|------|-----------|
| **Full** | Fetches all documents from the instance |
| **Incremental** | Fetches only documents modified since the last sync (state-driven via `last_sync_at`) |

Incremental state is checkpointed periodically so large syncs can resume.

## Configuration

Configured through the Omni settings page when adding a paperless-ngx source.

| Field | Description |
|-------|-------------|
| **Base URL** | URL of your paperless-ngx instance (e.g. `https://paperless.example.com`) |
| **API Key** | Paperless-ngx API token ([how to create one](https://docs.paperless-ngx.com/api/#authorization)) |

The API token must have read access to documents, tags, correspondents, document types, storage paths, and custom fields.
