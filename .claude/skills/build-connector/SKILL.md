---
name: build-connector
description: Build a new connector for Omni workplace search. Use when creating a new data source integration, scaffolding connector structure, or need help with the connector SDK.
argument-hint: <service name, e.g. "Asana", "Dropbox", "Zendesk">
user-invocable: true
---

You are helping build a connector for Omni, an open-source workplace search platform. Connectors are lightweight bridges between Omni and third-party APIs. Each connector runs as its own container.

# Core Principles

- **One container per connector.** Independently packaged and deployed.
- **Connectors NEVER interact directly with Omni services.** Everything goes through the connector-manager via the SDK.
- **SDKs are local dependencies** (not on PyPI/npm). All development happens in this monorepo.
- **Language choice is up to the developer.** Python, TypeScript, and Rust are all first-class.

**Sync flow:** connector receives `/sync` request -> fetches data from source API -> stores content via SDK -> emits document events -> calls `complete()`.

# Checklist

Every new connector requires changes across these areas:

1. **Connector implementation** — sync logic, API client, manifest
2. **Database migration** — add source type to `sources_source_type_check` constraint, add provider to `service_credentials_provider_check` if new
3. **Rust `SourceType` enum** — add variant to `shared/src/models.rs` `SourceType` enum (even for Python/TS connectors — the connector-manager is Rust)
4. **Frontend** — `SourceType` enum, icon, setup dialog, integrations page
5. **Docker Compose** — service definition, port in `.env.example`, `ENABLED_CONNECTORS` comment
6. **Terraform** — AWS ECS task/service, GCP Cloud Run service
7. **Integration tests** — mock API server, test harness, sync assertions

# Choosing a Language

| | Python | TypeScript | Rust |
|---|---|---|---|
| **SDK** | `sdk/python/` | `sdk/typescript/` | `shared/` crate |
| **Abstraction** | `Connector` base class, auto server/registration | `Connector<T,C,S>` base class, auto server/registration | Manual Axum server + `SdkClient` |
| **Test harness** | `omni_connector.testing` | Not yet available | Cargo test |
| **Reference connectors** | `connectors/notion/`, `connectors/hubspot/`, `connectors/linear/` | — | `connectors/google/`, `connectors/slack/`, `connectors/atlassian/` |
| **Simple example** | `sdk/python/examples/rss_connector.py` | `sdk/typescript/src/` | — |

When implementing, read the reference connectors for your chosen language. They are the canonical examples of project structure, Dockerfile, package config, and patterns.

# Connector Protocol

Every connector must expose these HTTP endpoints (Python/TS SDKs generate them automatically):

- `GET /health` — health check
- `GET /manifest` — return `ConnectorManifest` JSON
- `POST /sync` — trigger a sync (`{sync_run_id, source_id, sync_mode}`)
- `POST /cancel` — cancel a running sync (`{sync_run_id}`)
- `POST /action` — execute a custom action (`{action, params, credentials}`)

The connector auto-registers its manifest with connector-manager every 30 seconds (90s TTL).

# Manifest & Search Operators

The manifest describes the connector to the system. Key fields: `name`, `display_name`, `version`, `source_types` (must match frontend `SourceType` enum), `sync_modes` (`["full"]` or `["full", "incremental"]`), `search_operators`, `actions`.

**Search operators** let users filter results with keywords like `from:alice@example.com`:

- `operator` — the keyword users type (e.g., `"from"`, `"channel"`)
- `attribute_key` — document attribute to filter on (e.g., `"sender"`)
- `value_type` — `"person"`, `"text"`, or `"datetime"`

For operators to work, set matching keys in the document's `attributes` dict when emitting.

# Content Storage

Three methods for storing content, all return a `content_id` used when emitting documents:

1. **`save(content, content_type)`** — text/HTML
2. **`extract_and_store_content(data, mime_type, filename)`** — binary files (PDF, DOCX, etc.); connector-manager handles text extraction via Docling
3. **`save_binary(content, content_type)`** — raw binary as base64

# Emitting Documents

**Document** fields: `external_id` (stable ID from source), `title`, `content_id`, `metadata`, `permissions`, `attributes`.

- `metadata` — author, created_at, updated_at, url, mime_type, size, path, content_type, extra
- `permissions` — `public` (bool), `users` (email list), `groups` (group identifier list)
- `attributes` — key/value map for search operators and faceting. **NOT included in embeddings.**

**Event types:** `document_created`, `document_updated`, `document_deleted`, `group_membership_sync`.

# State Management

- Read previous state at sync start (passed as parameter)
- **Checkpoint periodically** during long syncs (`save_state()` — also sends heartbeat to prevent timeout)
- Save final state with `complete(new_state)`
- **Poll `is_cancelled()` in loops** — stop gracefully if cancelled
- Call `increment_scanned()` for progress tracking
- Check `should_index_user(email)` to respect whitelist/blacklist

Common pattern: store a cursor or timestamp (e.g., `{"last_sync_at": "2024-..."}`) for incremental syncs.

# Frontend Changes

These files need updating (read existing connectors in the frontend for patterns):

1. **`web/src/lib/types.ts`** — add to `SourceType` enum, `ServiceProvider` enum (if new provider), define config interface, add to `DEFAULT_SYNC_INTERVAL_SECONDS`
2. **`web/src/lib/utils/icons.ts`** — add SVG icon to `web/src/lib/images/icons/`, register in `SOURCE_TYPE_ICONS`, add display name
3. **`web/src/lib/components/`** — create setup dialog component (Dialog -> POST `/api/sources` -> POST `/api/service-credentials`)
4. **`web/src/routes/(admin)/admin/settings/integrations/+page.svelte`** — import and render setup component, add icon
5. **`web/src/routes/(admin)/admin/settings/integrations/+page.server.ts`** — add to `CONNECTOR_DISPLAY_ORDER`

# Docker Compose & ENABLED_CONNECTORS

Add connector service to `docker/docker-compose.yml` and dev overrides to `docker/docker-compose.dev.yml`. Follow the pattern of existing connectors.

Add a port variable to `.env.example` (next available after existing ports) and update the `ENABLED_CONNECTORS` comment.

**How ENABLED_CONNECTORS works:** The `.env` maps it directly to Docker Compose profiles:

```
ENABLED_CONNECTORS=google,slack,my-connector
COMPOSE_PROFILES=${ENABLED_CONNECTORS}
```

Only containers whose `profiles:` value appears in this list will start. Core services run regardless. By default only `web` is enabled.

# Terraform

Follow the pattern of existing connectors in:

- **AWS** (`infra/aws/terraform/modules/compute/`): add `aws_ecs_task_definition` and `aws_ecs_service` in `task_definitions.tf` and `services.tf`, gated by `count = contains(var.enabled_connectors, "name") ? 1 : 0`
- **GCP** (`infra/gcp/terraform/modules/compute/services.tf`): add to `all_simple_connectors` local map for simple connectors, or add count-based Cloud Run resource for complex ones

# Integration Testing

The Python test harness at `sdk/python/omni_connector/testing/` provides real ParadeDB, Redis, and connector-manager via testcontainers. See `connectors/notion/tests/` for the canonical test pattern:

1. Mock the third-party API (Starlette app in daemon thread)
2. Start connector server in daemon thread
3. Session-scoped `OmniTestHarness` starts infra + connector-manager
4. Function-scoped fixtures seed source/credentials pointing to mock
5. Trigger sync via `POST /sync` to connector-manager
6. Assert with `wait_for_sync()`, `count_events()`, `get_events()`

Run tests: `cd connectors/my-connector && uv run pytest` (Python) or `cargo test` (Rust).

# Key Files Reference

When implementing, read these files for your chosen language:

## Connector Implementation

| What | Python | TypeScript | Rust |
|---|---|---|---|
| Base class / SDK client | `sdk/python/omni_connector/connector.py` | `sdk/typescript/src/connector.ts` | `shared/src/sdk_client.rs` |
| Sync context (emit, state, storage) | `sdk/python/omni_connector/context.py` | `sdk/typescript/src/context.ts` | `shared/src/sdk_client.rs` (methods directly on `SdkClient`) |
| Content storage | `sdk/python/omni_connector/storage.py` | `sdk/typescript/src/storage.ts` | `SdkClient::store_content`, `extract_and_store_content` |
| Data models | `sdk/python/omni_connector/models.py` | `sdk/typescript/src/models.ts` | `shared/src/models.rs` |
| Server / registration | `sdk/python/omni_connector/server.py` | `sdk/typescript/src/server.ts` | Manual — see `connectors/google/src/main.rs` + `shared::start_registration_loop` |
| Package config | `connectors/notion/pyproject.toml` | `sdk/typescript/package.json` | `connectors/google/Cargo.toml` (workspace deps) |
| Dockerfile | `connectors/notion/Dockerfile` | — | `connectors/google/Dockerfile` (cargo-chef pattern) |
| Entry point | `connectors/notion/main.py` | — | `connectors/google/src/main.rs` |

## Backend (shared across all languages)

| What | File |
|---|---|
| Rust `SourceType` enum (must add variant) | `shared/src/models.rs` |
| Migration pattern for new source type | `services/migrations/075_add_paperless_ngx_source_type.sql` (latest example) |
| Migration directory | `services/migrations/` (files are numbered sequentially) |

## Frontend

| What | File |
|---|---|
| Source type & provider enums | `web/src/lib/types.ts` (`SourceType`, `ServiceProvider`, `DEFAULT_SYNC_INTERVAL_SECONDS`) |
| Icons & display names | `web/src/lib/utils/icons.ts` |
| Icon assets | `web/src/lib/images/icons/` |
| Setup dialog examples | `web/src/lib/components/*-setup.svelte` (e.g., `notion-setup.svelte`, `github-setup.svelte`) |
| Integrations page | `web/src/routes/(admin)/admin/settings/integrations/+page.svelte` |
| Display order | `web/src/routes/(admin)/admin/settings/integrations/+page.server.ts` (`CONNECTOR_DISPLAY_ORDER`) |

## Infrastructure

| What | File |
|---|---|
| Docker Compose services | `docker/docker-compose.yml` (connector definitions with `profiles:`) |
| Docker Compose dev overrides | `docker/docker-compose.dev.yml` |
| Port assignments & ENABLED_CONNECTORS | `.env.example` |
| AWS task definitions | `infra/aws/terraform/modules/compute/task_definitions.tf` |
| AWS ECS services | `infra/aws/terraform/modules/compute/services.tf` |
| GCP Cloud Run (simple connectors) | `infra/gcp/terraform/modules/compute/services.tf` (`all_simple_connectors` local) |

## Testing

| What | File |
|---|---|
| Test harness | `sdk/python/omni_connector/testing/harness.py` (`OmniTestHarness`) |
| DB seeding | `sdk/python/omni_connector/testing/seed.py` (`SeedHelper`) |
| Assertion helpers | `sdk/python/omni_connector/testing/assertions.py` (`wait_for_sync`, `count_events`, `get_events`) |
| Canonical test example | `connectors/notion/tests/conftest.py` + `connectors/notion/tests/test_full_sync.py` |

# Coding Guidelines

- Use concrete types, not `dict[str, Any]` / `Record<string, unknown>` / `serde_json::Value`
- No empty string `""` for missing state — use `None` / `null` / `Option`
- Fail immediately on missing required data
- Imports at top of file
- Only add comments explaining *why*, not *what*
- Prefer integration tests with real infrastructure over mocks
- Python: use `uv` (not pip)
