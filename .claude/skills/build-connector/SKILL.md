---
name: build-connector
description: Build a new connector for Omni workplace agents. Use when creating a new data source integration, scaffolding connector structure, exposing agent actions/tools/resources, or needing help with the connector SDK.
argument-hint: <service name, e.g. "Asana", "Dropbox", "Zendesk">
user-invocable: true
---

You are helping build a connector for Omni, an open-source AI agent platform for the workplace. Connectors are lightweight bridges between Omni and third-party systems: they sync workplace data into Omni's index and expose safe actions, MCP tools, resources, and prompts that workplace agents can use.

# Core Principles

- **One container per connector.** Independently packaged and deployed.
- **Connectors never interact directly with Omni services.** Everything goes through connector-manager via the SDK.
- **SDKs are local dependencies** in this monorepo, not published package registries.
- **Python, TypeScript, and Rust are first-class.** Pick the language that best fits the provider SDK/ecosystem.
- **Agents depend on connector contracts.** Model read/write actions, permissions, OAuth scopes, resources, prompts, and document IDs carefully.
- **Prefer first-party integrations.** For MCP, prefer a provider-managed Streamable HTTP server over a first-party self-hosted HTTP server, then over a first-party stdio/binary server. Use a community implementation only when no suitable first-party option exists and document the security and maintenance rationale.
- **Preserve the end-user security boundary.** User-initiated provider operations must use the invoking user's delegated credential whenever the provider supports it. Never silently substitute an org-wide credential.

**Scheduled sync flow:** connector-manager creates a sync run -> sends `POST /sync` to the connector -> connector fetches source config/credentials through the SDK -> connector fetches provider data -> stores/extracts content via SDK -> emits events -> checkpoints as it advances -> completes or fails through SDK.

**Realtime sync flow:** connector-manager starts/monitors a long-running `realtime` sync when the manifest declares support -> connector keeps a webhook/socket/watcher alive, heartbeats while idle, and emits repair/update events or creates short follow-up sync runs as needed.

# Checklist

Every new built-in connector usually requires changes across these areas:

1. **Provider discovery and design** — official APIs/SDKs/MCP, auth methods, tenant model, permissions, sync/change capabilities, rate limits, deletion history, and the decision to sync, expose live tools, or both.
2. **Connector implementation** — sync logic, provider client, config/credential types, manifest, actions/tools/resources/prompts/skills.
3. **Database migration** — add source type to `sources_source_type_check`; add service provider to `service_credentials_provider_check` if new.
4. **Rust `SourceType` enum** — add variant in `shared/src/models.rs` even for Python/TS connectors, because connector-manager is Rust.
5. **Frontend** — `SourceType`, `ServiceProvider` if new, icon, setup dialog, integrations page, OAuth/catalog state, sync interval defaults.
6. **Docker Compose** — service definition, dev override, port in `.env.example`, `ENABLED_CONNECTORS` comment.
7. **Terraform** — AWS ECS task/service and GCP Cloud Run service.
8. **GitHub Actions** — path filter, build job, release matrix entry.
9. **Integration tests** — mock provider API/MCP, connector-manager/testcontainer harness, sync/action/auth assertions.
10. **Operations and documentation** — least-privilege provider setup, credential rotation, reconnect/recovery, rate-limit behavior, known exclusions, and troubleshooting.

# Provider Discovery and Integration Design

Before coding, produce a short, source-linked capability report. Prefer current provider documentation over assumptions or existing community packages. Record:

- Official APIs and SDKs, supported versions, pagination, batch limits, rate limits, retry headers, and deprecations.
- Official managed or self-hosted MCP servers, transports, protocol versions, availability status, licensing, and supported tools/resources/prompts.
- OAuth authorization/token/userinfo/protected-resource metadata, PKCE, refresh/revocation, Dynamic Client Registration (DCR), service-account/API-key options, and tenant-specific endpoints.
- Whether OAuth/DCR requires an admin-created app, initial access token, confidential client secret, fixed callback URL, or source-scoped client registration.
- Full/incremental/realtime primitives: cursors, history APIs, timestamps, webhooks, replay guarantees, subscription renewal, deletion retention, and cursor expiry.
- Provider identity, tenant/account binding, users, groups, roles, inheritance, item ACLs, and how to verify the credential's principal.
- Data volume, attachment/export constraints, provider costs, expected sync interval, and which data is too sensitive, expensive, or volatile to index.

Choose and document one integration shape:

- native sync only;
- native live actions without sync;
- MCP-only remote integration; or
- hybrid metadata/content sync plus provider-managed MCP.

State exactly what is indexed and what remains live-only. If a provider-native MCP server exists, do not recreate its tool surface as native actions without a concrete reason. If its endpoint or catalog varies by connected `Source`, identify that before implementation: never hard-code a tenant endpoint or assume a connector-wide discovery result is valid for every source.

For third-party dependencies, use an official provider package when practical. If a community MCP/server/package is unavoidable, pin it, review its credential handling and transitive dependencies, verify its license, and record why first-party alternatives were unsuitable.

# Choosing a Language

| | Python | TypeScript | Rust |
|---|---|---|---|
| **SDK** | `sdk/python/` | `sdk/typescript/` | `sdk/rust/` crate `omni-connector-sdk` |
| **Abstraction** | `Connector` base class, FastAPI server/registration generated | `Connector<TConfig, TCredentials, TState>` base class, Express server/registration generated | `Connector` trait, Axum server/registration generated by SDK |
| **Typical sync state** | a concrete Pydantic/dataclass checkpoint model parsed at the boundary | generic concrete `TState` type | associated `type State: DeserializeOwned + Serialize` |
| **Test harness** | `omni_connector.testing` | SDK unit tests; no full connector harness yet | Cargo integration tests with connector-manager/testcontainers |
| **Reference connectors** | `connectors/notion/`, `connectors/github/`, `connectors/clickup/`, `connectors/microsoft/` | `connectors/linear/` | `connectors/google/`, `connectors/slack/`, `connectors/atlassian/`, `connectors/filesystem/`, `connectors/nextcloud/` |
| **Simple example** | `sdk/python/examples/rss_connector.py` | `sdk/typescript/examples/rss-connector.ts` | `connectors/filesystem/` for a compact Rust SDK connector |

When implementing, read reference connectors in the chosen language. They are the canonical examples of project structure, Dockerfile/package config, SDK usage, and sync/action patterns.

# Connector Protocol

The SDK server exposes these HTTP endpoints. Python/TypeScript/Rust SDKs generate them automatically unless you intentionally add extra routes in Rust with `serve_with_extra_routes`.

- `GET /health` — health check.
- `GET /manifest` — connector capabilities and agent-facing contract.
- `GET /sync/{sync_run_id}` — whether this connector process still has the sync in memory; connector-manager uses this to detect lost syncs and auto-resume.
- `POST /sync` — trigger or resume a sync. Payload includes `sync_run_id`, `source_id`, `sync_mode`, optional `checkpoint`, and `is_resume`.
- `POST /cancel` — cancel a running sync.
- `POST /action` — execute a connector action/tool. Connector-manager proxies the connector response status, headers, and body.
- `POST /resource` — read an MCP resource exposed by the connector.
- `POST /prompt` — get an MCP prompt exposed by the connector.

The connector auto-registers its manifest with connector-manager every 30 seconds. Registered manifests have a short TTL; connector-manager treats unregistered connectors as unavailable.

# Manifest

The manifest describes both sync capability and agent capability. Key fields:

- `name`, `display_name`, `version`, `description`
- `connector_id`, `connector_url`
- `source_types` — must match `shared/src/models.rs` `SourceType` and frontend `SourceType`.
- `sync_modes` — any subset of `full`, `incremental`, `realtime` that is truly implemented.
- `search_operators`
- `actions`
- `read_only` — Rust SDK supports connector-level read-only; connector-manager blocks write actions for read-only connectors/sources.
- `extra_schema`, `attributes_schema`
- `mcp_enabled`, `mcp_catalog_loaded`, `resources`, `prompts`
- `skills` — connector-owned instructions or MCP-prompt-backed skills.
- `oauth` — declarative OAuth2 config when the connector supports user-delegated OAuth.

A manifest is an authorization and routing contract, not only UI metadata. Do not advertise a sync mode, action, resource, prompt, or skill that cannot be dispatched safely. Treat an authenticated empty MCP catalog as authoritative; do not accidentally union it with stale tools. Conversely, distinguish a transient discovery failure from a successful empty catalog so a restart does not destroy a still-valid bounded cache without an intentional policy.

## Search Operators

Search operators let users filter results with keywords like `from:alice@example.com`.

- `operator` — keyword users type, e.g. `from`, `channel`, `status`.
- `attribute_key` — document attribute to filter on, e.g. `sender`, `channel_name`.
- `value_type` — usually `person`, `text`, or `datetime`.

For operators to work, emit matching keys in each document's `attributes`. Attributes are for filtering/faceting/tool routing; they are not embedded as document body text.

# Content Storage

Store content through connector-manager. Do not write directly to storage tables.

1. **`save(content, content_type)`** — text/HTML/Markdown, returns `content_id`.
2. **`extract_and_store_content(data, mime_type, filename)`** — binary files; connector-manager extracts text via Docling when enabled or built-in extractors, stores the extracted text, returns `content_id`.
3. **`extract_text(data, mime_type, filename)`** — same extraction but returns text without storing; use when you need to combine/post-process text before saving.
4. **`save_binary(content, content_type)`** — raw binary as base64, returns `content_id` (Python/TS helper; Rust can store text/extracted content directly).

Prefer `extract_and_store_content` for user-facing files and attachments. If extraction fails for a document but metadata is still useful, emit a metadata-only/fallback text document rather than silently dropping it.

# Emitting Documents and Events

Document fields:

- `external_id` — stable provider ID. This is what actions usually need after connector-manager resolves an Omni document ID.
- `title`
- `content_id`
- `metadata` — `title`, `author`, `created_at`, `updated_at`, `url`, `mime_type`, `size`, `path`, `content_type`, `extra`.
- `permissions` — `public`, `users`, `groups`.
- `attributes` — filter/facet/tool-routing metadata, not embedded body text.

Event types:

- `document_created`
- `document_updated`
- `document_deleted`
- `group_membership_sync`
- `person_sync`
- `person_deleted`

Use idempotent `external_id`/`document_id` values. Resume and retries can re-emit already completed units; indexer upserts/deletes should make that safe. Decide how a full sync reconciles provider deletions, objects removed from configuration scope, and permissions that became narrower. A crawl that only re-emits currently visible objects is not a complete full reconciliation if stale indexed documents can remain.

# Sync Modes, Checkpointing, and Resume

Always branch on the **dispatched sync mode**, not on whether state/checkpoint exists. A manual full sync can arrive with an existing checkpoint and must still perform full-crawl/full-reconciliation behavior. Connector-manager may upgrade the first scheduled incremental request for a source to `full` when there is no prior completed sync; honor the mode you receive.

## Full Sync

- Crawl all in-scope containers/items.
- Reconcile deletes, objects excluded by changed configuration, removed object types, and narrowed permissions; do not merely upsert the current live set.
- Decide whether full sync resets provider cursors and rebuilds users/groups/permission inheritance.
- If the provider cannot enumerate deletes or Omni cannot enumerate prior indexed IDs, define a bounded inventory/snapshot strategy or fail explicitly when safe reconciliation is impossible.
- Detect provider history-retention gaps. Never mark a run successful if the connector cannot prove it covered the interval needed for tombstones.
- For long crawls, checkpoint after each durable unit: user/mailbox, workspace, folder, channel, project, page-token range, etc.
- Do not skip a unit on resume unless the checkpoint proves it completed after all its events were flushed.

## Incremental Sync

- Prefer provider-native change cursors/history APIs.
- If only timestamps exist, use an overlap window and idempotent upserts/deletes to tolerate clock skew, late updates, provider reporting latency, and ordering quirks. Use a stable run cutoff so an interrupted run resumes the same logical window.
- Never advance a cursor/watermark before all items covered by it have been stored, emitted, and flushed.
- Include deletions and permission changes, not only content updates.
- Handle cursor expiry or history-retention gaps by falling back to an appropriate full/scoped repair, or fail explicitly if repair cannot be proven safe.
- Fingerprint configuration and permission inputs when their changes require broader reconciliation.
- Run periodic repair even when the incremental path is expected to be complete.

## Realtime Sync

Only declare `realtime` when the provider supports a reliable event path: webhooks, socket mode, file watchers, subscriptions, or equivalent.

Realtime connectors must:

- Keep the long-running watcher healthy and call `heartbeat()` while idle.
- Stop promptly when `ctx.is_cancelled()` flips.
- Reconcile event gaps with periodic incremental/full repair.
- Renew/verify provider subscriptions when applicable.
- Store subscription IDs, expirations, and last processed event times in source-level connector metadata when the SDK exposes that safely.
- Use `document_updated`/`document_deleted` for repair events when possible.

Current Rust SDK behavior to know:

- `SyncType::Realtime` occupies a separate per-source slot from `Full`/`Incremental`, so a realtime watcher does not block scheduled scans.
- Rust SDK auto-completes non-realtime syncs when `Connector::sync()` returns `Ok(())`; it does **not** auto-complete realtime syncs.
- For optional realtime support, implement `validate_sync_request()` and return `SyncRequestValidationError::Unavailable(...)` when prerequisites are missing. Connector-manager treats a realtime 404/unavailable as “not available”, not as a broken connector.

## Checkpoint and Resume Contract

Connector-manager maintains two different pieces of state:

- **Run checkpoint** (`sync_runs.checkpoint`) — saved by `save_checkpoint`; used to resume the same running sync after connector restart/loss.
- **Source checkpoint** (`sources.checkpoint`) — latest successful checkpoint; promoted from the run checkpoint only when the sync completes successfully.

Resume behavior:

- Fresh sync gets `checkpoint = source.checkpoint` and `is_resume = false`.
- Auto-resume gets `checkpoint = sync_run.checkpoint` if present, otherwise `source.checkpoint`, and `is_resume = true`.
- On `is_resume=true`, continue the same logical run: do not reset full-sync progress, do not advance provider cursors optimistically, and reprocess any in-progress unit idempotently.
- Failed/cancelled run checkpoints are not promoted to the source.
- Completion atomically marks the run completed and publishes the run checkpoint to the source.

Checkpointing rules:

- Define a concrete, versioned checkpoint model and reject malformed state at the boundary. Include dispatched mode, stable run cutoff, phase/unit/page progress, committed cursor/watermark, and relevant configuration/ACL fingerprints when needed.
- `save_checkpoint(...)` flushes buffered events before persisting the checkpoint in all SDKs. Use it after completed durable units.
- Never checkpoint past buffered/unflushed events.
- On resume, only skip units proven complete. Reprocess unfinished units idempotently.
- Poll cancellation in loops.
- Keep heartbeats fresh during long provider calls/page loops; checkpoint or explicit heartbeat is enough.
- Update progress counters as work completes. Use `increment_scanned` / `incrementScanned` / `increment_scanned(count)` for scanned items. In Rust and TypeScript, call `increment_updated` / `incrementUpdated` when you need the manager-side updated count to survive crashes.
- Do not promote a new provider cursor, ACL fingerprint, or scope fingerprint until every associated event and tombstone is durable and the run completes.

## Language-Specific State APIs

| Concern | Python | TypeScript | Rust |
|---|---|---|---|
| Dispatched mode | `ctx.sync_mode` (`SyncMode`) | `ctx.syncMode` (`SyncMode`) | `ctx.sync_mode()` (`SyncType`) |
| Resume flag | `ctx.is_resume` | `ctx.isResume` | `ctx.is_resume()` |
| Current checkpoint | `checkpoint` arg and `ctx.checkpoint` | `state` arg and `ctx.state` | `state: Option<Self::State>` arg |
| Save checkpoint | `await ctx.save_checkpoint({...})` | `await ctx.saveCheckpoint({...})` / `saveState` alias | `ctx.save_checkpoint(json).await?` |
| Complete | `await ctx.complete(checkpoint={...})` | `await ctx.complete({...})` | normally return `Ok(())` after saving checkpoint; `ctx.complete().await?` is also available |
| Cancel check | `ctx.is_cancelled()` | `ctx.isCancelled()` | `ctx.is_cancelled()` |
| Mark cancelled | no public cancel helper; current connectors generally stop/fail | no public cancel helper | `ctx.cancel().await?` |
| Source-level connector metadata | `ctx.connector_state`, `ctx.save_connector_state(...)` | `SdkClient.updateConnectorState(...)`; `SyncContext` does not expose a helper today | `SdkClient::get_connector_state` / `save_connector_state`; do **not** use deprecated `SyncContext::save_connector_state` for source metadata because it aliases checkpoint |

# Actions and Agent Tools

Actions are how connectors expose provider capabilities to Omni agents and setup/UI flows. Define them deliberately.

`ActionDefinition` fields:

- `name` — stable action name.
- `description` — agent-facing description; explain when to use it and important IDs/side effects.
- `input_schema` — JSON Schema object for parameters. Keep it strict and concrete.
- `mode` — `read` or `write`. Default is write in shared/Rust, so set read explicitly for non-mutating actions. Unknown MCP tools remain write.
- `credential_scope` — `user` by default or deliberately `org` for setup/org-wide operations.
- `required_scopes` — action-level OAuth scopes when known; `None` means only coarse credential checks are possible.
- `source_types` — restrict action to specific source types. Empty means all source types for that connector.
- `admin_only` — restricts callers to admins and resolves the org credential.
- `actor_scoped` — only for a write whose connector implementation derives/enforces the affected provider identity from the trusted actor; it cannot be combined with `admin_only`, org `credential_scope`, or read mode.
- `hidden` — hidden from all chat/agent tool listings but still present in the manifest and dispatchable by name for setup/internal flows.
- `origin` — `native` or `mcp`; discovered tools must retain MCP provenance.

Connector-manager behavior:

- `/actions` excludes hidden actions and write actions blocked by connector/source `read_only`.
- `execute_action` blocks write actions for read-only connectors or sources.
- User-scoped agent calls require an acting user. If the manifest supports user OAuth, the caller's credential is mandatory; when missing, connector-manager returns `412 needs_user_auth` with an OAuth start URL. It must not fall back to the org credential.
- `admin_only` and explicit org-scoped actions use the source org credential. A non-`actor_scoped` write on an org-credential-only provider is admin-only at dispatch.
- Source config is merged into action params, with caller params taking precedence; connectors must still use the trusted `Source` object for security-sensitive routing and bindings.
- `document_id` / `file_id` params may be resolved from Omni document ID to provider `external_id` before dispatch.
- Connector-manager proxies the connector's response status, headers, and body. Actions may return JSON or binary/file responses.

Action implementation rules:

- Declaring manual `actions` only advertises them; override `execute_action` / `executeAction` / `execute_action(...)` to implement dispatch. The base classes only handle MCP tools and otherwise return not supported.
- Validate required params and credential shape immediately.
- Return `ActionResponse.success(...)`, `failure(...)`, or `not_supported(...)` for JSON actions.
- For file/download actions, return a real HTTP response with content type and filename headers.
- Prefer source-native IDs in action params, but document in the schema when Omni document IDs are accepted/resolved.
- Keep write actions narrow and safe; mark `mode="write"` and require user OAuth where appropriate.

# MCP, Resources, Prompts, and OAuth

SDKs can bridge an external MCP server into the connector protocol.

## MCP implementation choice

Use this order unless provider constraints make it impossible:

1. Provider-managed first-party Streamable HTTP.
2. First-party self-hosted Streamable HTTP.
3. First-party stdio/binary.
4. Vetted community server with documented justification.

Prefer HTTP over spawning a CLI, npm package, or downloaded binary. Do not choose a random community package when the provider exposes an official endpoint. Record the upstream owner, URL, availability status, protocol revision, authentication method, license where relevant, and tool categories.

Before integration, determine whether the endpoint and catalog are global, deployment-specific, account-specific, or source-specific. A connector may handle multiple `sources` rows; never route every source to the first configured endpoint. If upstream servers can contribute to one connector manifest, define deterministic union/deduplication and same-name/schema-conflict behavior. Fail closed on ambiguous contracts.

## MCP adapter and catalog

To enable MCP:

- Override `mcp_server` / `mcp_server()` with stdio or Streamable HTTP config when the endpoint is truly connector-wide; use a source-aware platform path when it is not.
- Implement `prepare_mcp_env(...)` for stdio auth or `prepare_mcp_headers(...)` for HTTP auth.
- SDK bootstrap discovers MCP tools/resources/prompts after credentials are available.
- MCP tools are merged into manifest `actions`; MCP resources/prompts populate manifest `resources`/`prompts` and are served through `/resource` and `/prompt`.
- Preserve `origin="mcp"`. Accept explicit trustworthy read-only annotations; otherwise classify tools as write.
- Treat tool names, descriptions, and schemas as untrusted provider data: bound counts/sizes, reject unsupported schemas, and account for tool poisoning/shadowing.
- Define catalog cache ownership, authenticated bootstrap, refresh interval, restart recovery, source/config invalidation, transient-failure behavior, and bounded staleness.
- A successful authenticated empty catalog removes stale capabilities. A network/auth failure is not an empty catalog.
- If discovery requires OAuth, provide an explicit setup/settings OAuth CTA; do not create a deadlock where no tool is visible and OAuth can only start after tool invocation.
- Catalog discovery may use one consenting credential solely for list operations, but tool/resource/prompt execution must still use the invoking user's credential wherever delegated auth is supported.

## OAuth and credential policy

Python, TypeScript, and Rust SDKs expose declarative `OAuthManifestConfig` support.

- Declare provider auth/token/userinfo endpoints, identity scopes, per-source-type read/write scopes, extra auth params, scope separator, optional enrich endpoint, PKCE, token auth method, resource indicator, grant types, endpoint validation, and org-OAuth support accurately.
- Investigate DCR first. When supported, declare `registration_endpoint`, whether an initial access token is required, public/confidential client behavior, and returned secret expiry. When unsupported, require the administrator to configure a provider OAuth client in settings.
- Tenant/account-specific issuers or endpoints require source-scoped OAuth client configuration. Never let one source silently use another tenant's client or endpoint.
- Use the narrowest read scopes for sync/read actions and add write scopes only for write tools. If a provider uses roles rather than distinct read/write scopes, retain Omni's write classification/approval and document that provider RBAC is the authority.
- User-context actions, tools, resources, and prompts require the invoking user's credential whenever supported. Missing user auth must produce `needs_user_auth`; org fallback is forbidden.
- Org credentials may be used for sync, hidden setup actions, and explicit org/admin operations. Exceptional org-credential user writes are allowed only when delegated auth is unavailable and `actor_scoped` enforcement is narrow, server-side, and tested.
- Implement access-token refresh/expiry/reconnect behavior and ensure refresh failures cannot cause org-credential fallback.

## OAuth credential validation and source binding

Do not trust tenant IDs, account URLs, principal emails, or similar claims merely because they appear in source config, OAuth state metadata, or a token response.

- Use the fresh credential against a provider endpoint to verify the account/tenant and principal.
- Return a stable `OAuthSourceBinding` and reject credentials for a different connected source account.
- Compare the provider-verified principal with the trusted authenticated Omni user for delegated flows. If the provider has no userinfo endpoint, use another provider-authenticated identity API or add a narrowly scoped validation path; do not silently bind an unverifiable identity.
- Validate dynamic issuer/resource/endpoint URLs against SSRF, redirect, DNS rebinding, credential-in-URL, scheme, port, and response-size threats.
- Test org-source, connect-source, user-read, and user-write flows independently when supported.

# Permissions and Inheritance

- Model the provider's authorization semantics before choosing document granularity.
- Identify whether permissions are item-level, container-inherited, mailbox-owned, workspace-wide, role/group-derived, directly user-granted, or affected by nested roles/groups and secondary roles.
- Emit `permissions.users` and `permissions.groups` using stable identifiers already understood by Omni's permission filter.
- If permissions inherit from a parent container, store parent/container IDs in metadata/attributes and update affected child documents when membership changes.
- If the provider supports group/role principals, emit `group_membership_sync` events and expand inheritance exactly. Protect against cycles and exclude disabled/deleted principals.
- Do not equate a provider's “public/everyone” role with Omni `permissions.public=true` unless every Omni user is provably in the provider audience. Prefer an explicit provider-members group.
- Reconcile revoked grants and removed memberships promptly. Permission widening must never be used as a fallback for incomplete ACL data; fail closed.
- Be conservative with private/DM/personal content. Prefer allowlists and opt-in handling until ACL semantics are proven.
- Use `ctx.should_index_user(...)` / `ctx.shouldIndexUser(...)` where available before emitting per-user records under source whitelist/blacklist settings.

# Document Modeling, Threads, and Attachments

- Use stable provider IDs for `external_id`; avoid per-crawl or user-local IDs unless the source truly has user-local objects.
- Decide document granularity deliberately: item, message, thread, day/channel batch, file attachment, etc. This affects dedupe, incremental sync, permissions, snippets, and retrieval.
- Preserve parent references in metadata/attributes: `container_id`, `thread_id`, `parent_message_id`, `attachment_id`, etc.
- For threaded systems, decide whether to index each message separately, group by thread, or re-emit the full thread on changes. Implement incremental/realtime repair accordingly.
- For attachments, distinguish linked provider files from uploaded blobs. Prefer extracting/storing attachment content as its own document when independently useful, with a back-reference to the parent item.
- For unsupported/oversized/private attachments, emit metadata-only documents or pointers rather than silently dropping them.

# Frontend Changes

Read existing connector patterns before editing. Typical files:

1. **`web/src/lib/types.ts`** — `SourceType`, `ServiceProvider` if new, source config interface, `DEFAULT_SYNC_INTERVAL_SECONDS`.
2. **`web/src/lib/utils/icons.ts`** — register SVG icon and display name.
3. **`web/src/lib/images/icons/`** — icon asset.
4. **`web/src/lib/components/*-setup.svelte`** — setup dialog component.
5. **`web/src/routes/(admin)/admin/settings/integrations/+page.svelte`** — import/render setup component.
6. **`web/src/routes/(admin)/admin/settings/integrations/+page.server.ts`** — `CONNECTOR_DISPLAY_ORDER`.

## Icon Sourcing

- Fetch a local SVG icon for the app/service; do not hotlink remote assets.
- Prefer Wikimedia Commons when available because it usually has stable SVG files and explicit license metadata. Use the original SVG file URL, not a rendered PNG thumbnail.
- If Wikimedia does not have a suitable official mark, use the provider's official brand/media kit or another reputable source with compatible licensing. Record the source URL in your summary/PR notes.
- Verify the asset license before committing. Do not add icons with unclear, incompatible, or non-redistributable terms.
- Save the SVG under `web/src/lib/images/icons/` using the existing naming style, then register it in `web/src/lib/utils/icons.ts`.
- Keep the SVG compact and safe: no scripts, external references, embedded rasters, tracking metadata, or remote font/image links.

# Docker Compose and ENABLED_CONNECTORS

Add connector service to `docker/docker-compose.yml` and dev overrides to `docker/docker-compose.dev.yml`. Follow existing connectors.

Add a port variable to `.env.example` and update the `ENABLED_CONNECTORS` comment.

`ENABLED_CONNECTORS` maps directly to Docker Compose profiles:

```env
ENABLED_CONNECTORS=google,slack,my-connector
COMPOSE_PROFILES=${ENABLED_CONNECTORS}
```

Only containers whose `profiles:` value appears in this list start. Core services run regardless.

# Terraform

Follow existing connector patterns:

- **AWS** (`infra/aws/terraform/modules/compute/`): add task definition and ECS service in `task_definitions.tf` and `services.tf`, gated by `count = contains(var.enabled_connectors, "name") ? 1 : 0`.
- **GCP** (`infra/gcp/terraform/modules/compute/services.tf`): add to `all_simple_connectors` for simple connectors, or add a count-based Cloud Run resource for complex ones.

# Reliability, Limits, and Observability

- Parse provider responses into concrete boundary models and fail explicitly on missing required fields.
- Honor provider `Retry-After` and documented quota headers. Use exponential backoff with jitter, bounded attempts, and bounded concurrency; distinguish retryable transport/429/5xx errors from permanent auth/schema/permission errors.
- Keep requests, pages, downloads, MCP schemas, and responses size-bounded. Stream large payloads where supported.
- Use deterministic pagination ordering where possible. Detect repeated cursors/pages to prevent infinite loops.
- Never log access tokens, refresh tokens, API keys, private keys, authorization headers, raw credential payloads, or secret-bearing provider error bodies. Redact URLs that may contain secrets.
- Emit actionable errors with source/unit context but no secrets. Decide whether an item failure can be emitted and skipped or must fail the whole run based on consistency and permission risk.
- Add health/setup validation that proves account identity and required privileges, not merely TCP reachability.

# Integration Testing

Prefer integration tests with real connector-manager/Postgres/Redis infrastructure.

Python harness at `sdk/python/omni_connector/testing/` provides ParadeDB/Postgres, Redis, and connector-manager via testcontainers. See `connectors/notion/tests/`, `connectors/github/tests/`, and `connectors/clickup/tests/`.

Typical Python pattern:

1. Mock third-party API with a Starlette app in a daemon thread.
2. Start connector server in a daemon thread.
3. Session-scoped `OmniTestHarness` starts infra + connector-manager.
4. Function-scoped fixtures seed source/credentials pointing to mock API.
5. Trigger sync via connector-manager `POST /sync`.
6. Assert with `wait_for_sync()`, `count_events()`, `get_events()`, source checkpoint helpers, and action calls.

Rust connectors use Cargo integration tests with connector-manager/testcontainers; see `connectors/slack/tests/`, `connectors/web/tests/`, and `connectors/atlassian/tests/`.

Test these behaviors when relevant:

- full sync from empty state
- manual full sync when a checkpoint already exists
- incremental sync from checkpoint, including overlap/cutoff and deletion paths
- checkpoint promotion only on success and after buffered events flush
- resume with `is_resume=true` from each durable phase/page boundary
- cancellation
- configuration narrowing and provider history/cursor expiry
- action schema/listing/dispatch, including hidden/admin/read-only/actor-scoped behavior
- required OAuth scopes and read/write approval behavior
- missing per-user auth returns `needs_user_auth` and never falls back to org credentials
- OAuth refresh/expiry/reconnect, DCR or manual client setup, source/account binding, and wrong-principal rejection
- authenticated MCP bootstrap, empty catalog, refresh failure, restart/cache recovery, source routing, conflicting tools, and write-default classification
- permissions, nested role/group membership, revocation, disabled users, and fail-closed missing ACL data
- rate limits, retries, repeated cursors, response bounds, SSRF checks, and secret redaction
- realtime watcher heartbeat/cancel/unavailable/gap-repair path

Run examples:

```bash
cd connectors/my-python-connector && uv run pytest
cargo test -p omni-my-connector
```

# Key Files Reference

## Connector SDKs

| What | Python | TypeScript | Rust |
|---|---|---|---|
| Base class / trait | `sdk/python/omni_connector/connector.py` | `sdk/typescript/src/connector.ts` | `sdk/rust/src/connector.rs` |
| Sync context | `sdk/python/omni_connector/context.py` | `sdk/typescript/src/context.ts` | `sdk/rust/src/context.rs` |
| SDK client | `sdk/python/omni_connector/client.py` | `sdk/typescript/src/client.ts` | `sdk/rust/src/client.rs` |
| Content storage | `sdk/python/omni_connector/storage.py` | `sdk/typescript/src/storage.ts` | methods on Rust `SyncContext`/`SdkClient` |
| Data models | `sdk/python/omni_connector/models.py` | `sdk/typescript/src/models.ts` | `sdk/rust/src/models.rs` + `shared/src/models.rs` |
| Server / registration | `sdk/python/omni_connector/server.py` | `sdk/typescript/src/server.ts` | `sdk/rust/src/server.rs` |
| MCP adapter | `sdk/python/omni_connector/mcp_adapter.py` | `sdk/typescript/src/mcp-adapter.ts` | `sdk/rust/src/mcp_adapter.rs` |
| Simple example | `sdk/python/examples/rss_connector.py` | `sdk/typescript/examples/rss-connector.ts` | `connectors/filesystem/` |

## Backend Shared Files

| What | File |
|---|---|
| Source types, manifests, action definitions, sync types/events | `shared/src/models.rs` |
| Connector-manager sync orchestration/resume | `services/connector-manager/src/sync_manager.rs` |
| Connector-manager action/resource/prompt dispatch | `services/connector-manager/src/handlers.rs` |
| Connector HTTP client | `services/connector-manager/src/connector_client.rs` |
| Scheduler and realtime startup | `services/connector-manager/src/scheduler.rs` |
| Migration pattern for source type | `services/migrations/075_add_paperless_ngx_source_type.sql` or latest source-type migration |
| Migration directory | `services/migrations/` |

## Frontend

| What | File |
|---|---|
| Source type & provider enums | `web/src/lib/types.ts` |
| Icons & display names | `web/src/lib/utils/icons.ts` |
| Icon assets | `web/src/lib/images/icons/` |
| Setup dialog examples | `web/src/lib/components/*-setup.svelte` |
| Integrations page | `web/src/routes/(admin)/admin/settings/integrations/+page.svelte` |
| Display order | `web/src/routes/(admin)/admin/settings/integrations/+page.server.ts` |

## Infrastructure and CI

| What | File |
|---|---|
| Docker Compose services | `docker/docker-compose.yml` |
| Docker Compose dev overrides | `docker/docker-compose.dev.yml` |
| Port assignments & ENABLED_CONNECTORS | `.env.example` |
| AWS task definitions | `infra/aws/terraform/modules/compute/task_definitions.tf` |
| AWS ECS services | `infra/aws/terraform/modules/compute/services.tf` |
| GCP Cloud Run | `infra/gcp/terraform/modules/compute/services.tf` |
| Path filters/build/release matrix | `.github/workflows/ci.yml` |

## Testing

| What | File |
|---|---|
| Python test harness | `sdk/python/omni_connector/testing/harness.py` |
| Python DB seeding | `sdk/python/omni_connector/testing/seed.py` |
| Python assertions | `sdk/python/omni_connector/testing/assertions.py` |
| Python examples | `connectors/notion/tests/`, `connectors/github/tests/`, `connectors/clickup/tests/` |
| Rust examples | `connectors/slack/tests/`, `connectors/web/tests/`, `connectors/atlassian/tests/` |

# Definition of Done

Before calling a connector complete, verify:

- The provider discovery report and sync/MCP/indexing decisions are recorded.
- All supported modes and advertised capabilities work end to end; unsupported modes are not declared.
- Full/incremental/realtime (as applicable), deletions, permission revocation, config changes, resume, cancellation, and repair are covered.
- The auth matrix proves every user-facing operation uses the intended principal and least privilege.
- OAuth bootstrap, refresh, tenant/principal validation, and catalog recovery work after service restarts.
- Setup UI reports missing privileges/auth/catalog state with actionable remediation.
- Source type/provider migrations, shared enums, frontend, icon/license, Compose, Terraform, CI, and release matrix are updated.
- A README/runbook documents minimum provider privileges, callback URLs, credential rotation, reconnect, rate limits, expected latency, exclusions, and troubleshooting.
- Unit/type/lint checks and connector-manager-backed integration tests pass.

# Coding Guidelines

- Use concrete types, not `dict[str, Any]` / `Record<string, unknown>` / `serde_json::Value`, when the shape is known. Opaque maps are acceptable only at SDK boundaries or for genuinely provider-dynamic payloads.
- No empty string `""` for missing state; use `None` / `null` / `Option`.
- Fail immediately on missing required config/credentials.
- Imports at the top of the file.
- Only add comments explaining why, not what.
- Prefer integration tests with real infrastructure over isolated mocks.
- Python: use `uv`, not `pip`, for connector development in this repo.
- Svelte buttons must use Tailwind `cursor-pointer`.
