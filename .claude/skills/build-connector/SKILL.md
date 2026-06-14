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
7. **GitHub Actions** — path filter, build job, and release matrix entry
8. **Integration tests** — mock API server, test harness, sync assertions

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

Four methods for storing/extracting content, all go through the connector-manager:

1. **`save(content, content_type)`** — text/HTML, returns `content_id`
2. **`extract_and_store_content(data, mime_type, filename)`** — binary files (PDF, DOCX, etc.); connector-manager extracts text via Docling (when enabled) or built-in extractor, stores result, returns `content_id`
3. **`extract_text(data, mime_type, filename)`** — same extraction as above but returns the extracted text without storing; use when the caller needs to post-process or combine text before storing (e.g., appending attachment text to an email thread body)
4. **`save_binary(content, content_type)`** — raw binary as base64, returns `content_id`

# Emitting Documents

**Document** fields: `external_id` (stable ID from source), `title`, `content_id`, `metadata`, `permissions`, `attributes`.

- `metadata` — author, created_at, updated_at, url, mime_type, size, path, content_type, extra
- `permissions` — `public` (bool), `users` (email list), `groups` (group identifier list)
- `attributes` — key/value map for search operators and faceting. **NOT included in embeddings.**

**Event types:** `document_created`, `document_updated`, `document_deleted`, `group_membership_sync`.

# State Management

- Read previous state at sync start (passed as parameter)
- **Checkpoint periodically** during long syncs (`save_state()` / `save_checkpoint()` — also sends heartbeat to prevent timeout)
- Save final state with `complete(new_state)` / final checkpoint promotion
- **Poll `is_cancelled()` in loops** — stop gracefully if cancelled
- Call `increment_scanned()` for progress tracking
- Check `should_index_user(email)` to respect whitelist/blacklist

Common pattern: store a cursor or timestamp (e.g., `{"last_sync_at": "2024-..."}`) for incremental syncs.

## Sync Modes, Resume, and Checkpointing

For every connector, explicitly decide and document whether each mode is supported:

- **Full sync** — crawl all in-scope containers/items. For long crawls, checkpoint after each completed durable unit (user, folder, channel/space, page token, etc.) so resume can skip completed work without skipping unfinished work.
- **Incremental sync** — prefer provider-native change cursors/events. If only timestamp filtering exists, use an overlap window and idempotent upserts/deletes to tolerate clock skew and late updates. Store enough state to resume in the middle of a page and to recover when a provider cursor expires.
- **Realtime sync** — only claim realtime support when provider webhooks/events can be renewed, verified, replayed/caught up after outages, and reconciled with periodic incremental/full repair. Store webhook/subscription IDs, expirations, and last processed event times in source-level connector state.

Checkpoint design:

- Separate **source-level durable state** (provider cursors, webhook subscriptions, final watermarks) from **per-run resume state** (currently processed container/page/user, partial page token, completed unit set).
- Save checkpoints after every completed unit and before/after long page loops where safe.
- Never advance a cursor/watermark before all items covered by it have been emitted and flushed.
- On cancel, persist completed work and mark the run cancelled rather than failed.
- On resume, only skip units proven complete by checkpoint state; unfinished units must be reprocessed idempotently.

## Permissions and Inheritance

- Model the provider's authorization semantics before choosing document granularity.
- Identify whether permissions are item-level, container-inherited, user-mailbox-owned, or group-derived.
- Emit `permissions.users` and `permissions.groups` using stable identifiers already understood by Omni's permission filter.
- If permissions inherit from a parent container, store the parent/container ID in document metadata/attributes and update affected child documents when membership changes.
- If the provider supports group principals, reuse existing group membership sync where possible; otherwise add group sync events or resolve groups safely.
- Be conservative with private/DM/personal content. Prefer allowlists and opt-in handling until ACL semantics are proven.

## Document Modeling, Threads, and Attachments

- Use stable provider IDs for `external_id`; avoid per-crawl/user-local IDs unless the source truly has user-local objects.
- Decide document granularity deliberately: single message/item, thread, daily channel batch, file attachment, etc. The choice affects dedupe, incremental sync, permissions, snippets, and retrieval.
- Preserve parent references in metadata/attributes (`container_id`, `thread_id`, `parent_message_id`, `attachment_id`) so agents can navigate from search hits to the original context.
- For threaded systems, decide whether to index each message separately, group by thread, or re-emit the full thread on changes. Implement incremental sync accordingly.
- For attachments, distinguish linked provider files from uploaded blobs. Prefer extracting/storing attachment content as its own document when it is independently useful, and include a back-reference to the parent message/thread/document.
- For unsupported/oversized/private attachments, emit metadata-only documents or attachment pointers rather than silently dropping them.

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
| Content storage | `sdk/python/omni_connector/storage.py` | `sdk/typescript/src/storage.ts` | `SdkClient::store_content`, `extract_and_store_content`, `extract_text` |
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

## CI/CD

| What | File |
|---|---|
| Path filter | `.github/workflows/ci.yml` (`detect-changes` → `filters`) |
| Build job | `.github/workflows/ci.yml` (connector build job calling `build-connector.yml`) |
| Release matrix | `.github/workflows/ci.yml` (`release-connectors` matrix) |

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
