# Salesforce Connector Revival — Plan

Branch `feature/salesforce-connector` has drifted **386 commits** behind `origin/master`.
The staged salesforce connector was written against a contract that no longer exists.
This plan covers (a) contract re-baselining, (b) the six functional review areas,
(c) strong typing throughout.

---

## 1. Contract drift found (vs `origin/master`)

### 1.1 Python SDK (`sdk/python/omni_connector`) — what changed

| Old (staged code assumes) | Current contract on master |
|---|---|
| `sync(source_config, credentials, state, ctx)` | `sync(source_config, credentials, checkpoint: dict \| None, ctx)` — third arg is the **resume checkpoint** |
| `ctx.complete(new_state={})` | `ctx.complete(checkpoint=None)` — `new_state=` kwarg no longer exists (would crash) |
| `source_types` optional | `source_types` is an **`@abstractmethod` property** — connector without it fails at registration |
| sync_modes `["full"]` only | `SyncMode` enum: `FULL`/`INCREMENTAL`/`REALTIME`; per-mode event-buffer thresholds (full=500/300s, incremental=100/60s, realtime=1/none) |
| `SyncContext` state | `ctx.checkpoint`, `ctx.connector_state`, `ctx.save_checkpoint()`, `ctx.save_connector_state()`, `ctx.sync_mode`, `ctx.is_resume`, `ctx.emit_updated()`, `ctx.emit_deleted()`, `ctx.emit_group_membership()`, `ctx.emit_person_sync()`, `ctx.emit_person_deleted()` |
| Manifest: name/version/sync_modes/actions only | Manifest now also carries `search_operators`, `skills`, `oauth` (`OAuthManifestConfig`), `extra_schema`, `attributes_schema`, `mcp_*` fields |
| Events: document_created only | `ConnectorEvent` union: document_created/updated/deleted + `group_membership_sync` + `person_sync`/`person_deleted` |
| `DocumentPermissions(public=True)` | `{public, users[], groups[]}` — users/groups are **emails**; indexer/searcher filter on them |
| — | `SyncRequest` carries `sync_mode`, `checkpoint`, `is_resume`, seeded `documents_scanned/updated` counts |

### 1.2 Rust shared + services

- `shared/src/models.rs` `SourceType` has 26 variants (incl. `Darwinbox`, `Windshift`) — **no `Salesforce`**. Must add variant + `as_str()` + `ServiceProvider::Salesforce`.
- Migration numbering: master is at **111**. Staged `057_add_salesforce_source_type.sql` is stale in both number and shape — the constraint now has an `integration_type` column (connector vs `remote_mcp` regex). New file must be **`112_...`** copying the windshift (108) constraint shape.
- `services/connector-manager/src/config.rs`: staged `SALESFORCE_CONNECTOR_URL` insertion still matches master's per-connector env pattern — re-apply cleanly.
- `services/searcher/src/query_parser.rs`: `in:` operator alias list (`resolve_source_alias`) — add `salesforce` (master recently did this for darwinbox).
- Exhaustive `SourceType` matches elsewhere (connector-manager handlers/scheduler, searcher) — compiler will flag; fix all.

### 1.3 Web

- `web/src/lib/types.ts` on master has many new enum values (`MS_TEAMS`, `IMAP`, `CLICKUP`, `LINEAR`, `PAPERLESS_NGX`, `NEXTCLOUD`, `GOOGLE_ADS`, `DARWINBOX`, `WINDSHIFT`, `IntegrationType`, `DEFAULT_SYNC_INTERVAL_SECONDS`). Staged diff was against the old enum — must be rebased, plus:
  - `DEFAULT_SYNC_INTERVAL_SECONDS[SourceType.SALESFORCE]` (3600)
  - `web/src/lib/utils/sources.ts` — `getSourceNoun` entry (`records`)
- `+page.server.ts` / `+page.svelte` (integrations): staged hunks apply with context drift; master page has more connectors/imports — merge carefully.
- `salesforce/[sourceId]/+page.server.ts` + `.svelte`: staged versions written against an older base — rewrite to match master's `hubspot/[sourceId]` pages.
- `salesforce-connector-setup.svelte`: verify against master's `hubspot-connector-setup.svelte` (authType/credentials API shape).
- Icon SVG is fine.

### 1.4 Docker / env

- `.env.example`, `docker/docker-compose.yml` (+dev): staged additions are correct in shape but must be re-applied onto master's files (many new connectors/profiles since).

---

## 2. Review findings — the six areas

### 2.1 Sync logic — GAP
Current: `sync_modes = ["full"]` only; no incremental, no realtime.

**Implement:**
- **full**: as today, but with checkpointing + all entities (see 2.2, 2.5).
- **incremental**: per-object SOQL `WHERE SystemModstamp > :watermark ORDER BY SystemModstamp, Id` with keyset pagination (Salesforce `OFFSET` is capped at 2000 — keyset is the only reliable paging for delta queries). Overlap window (e.g. re-scan last 15 min of watermark) to catch updates that landed mid-sync / clock skew. Deletes via REST `sobjects/{type}/deleted?start&end` → `ctx.emit_deleted(external_id)`. Updates via `ctx.emit_updated()` (not emit) so indexer can cheaply update.
- **realtime**: advertise `realtime` and implement a **long-lived polling sync** (realtime slots are supervised by the scheduler; buffer flushes every event). Poll `GetUpdated`/`GetDeleted` windows every N seconds (config, default ~60s), emit created/updated/deleted. (Salesforce CDC/CometD streaming is possible but heavy; polling is robust, uses the same REST paths as incremental, and needs no new deps.)

### 2.2 Checkpointing — GAP (critical)
Current: no checkpointing at all (`complete(new_state={})`), every sync redoes everything.

**Implement** a typed `SalesforceCheckpoint` (clickup-style, see `connectors/clickup/.../models.py`):
- `watermarks: dict[ObjectType, datetime]` — max `SystemModstamp` per object type (incremental cursor).
- `progress: dict[ObjectType, ObjectProgress]` — mid-sync resume state per object: phase (`records`/`users`/`groups`/`shares`), keyset `(last_system_modstamp, last_id)`, plus per-entity progress (`shares` keyset etc.).
- Save **periodically** (every N records / per page) via `ctx.save_checkpoint()` (flushes events first — crash-safe ordering).
- Write a run-scoped checkpoint immediately at sync start (clickup pattern) so a crash before the first page doesn't fall back to stale watermarks.
- **Watermark invalidation**: store the *synced field-set + object list + API version* in `connector_state` (via `ctx.save_connector_state()`). On every sync, compare against current `config.py`; if anything changed (fields added/removed, object added/removed, connector version bump, API version bump) → drop watermarks, run full sync, persist new field-set. Also invalidate on `SystemModstamp` watermark going backwards (e.g. data restore).
- Deleted-record watermark: track last `deleted` window end too.

### 2.3 Attributes — GAP
Current: only `source_type`, `object_type`, `salesforce_id` — not enough for operators or filtering.

**Implement:** emit structured attributes for every object type from its typed record:
- common: `object_type`, `salesforce_id`, `owner_id`, `owner_email`, `created_date`, `modified_date` (ISO strings)
- per-type: Account → `industry`, `account_type`, `billing_country`, `annual_revenue`; Opportunity → `stage`, `amount`, `close_date`, `probability`; Case → `case_status`, `priority`, `case_type`, `origin`; Lead → `lead_status`, `lead_source`; Contact → `account_name`, `department`, `title`; Task → `task_status`, `priority`, `activity_date`, `related_to_type`
- Declare `attributes_schema` in the manifest (JSON Schema) — indexer's people_extractor uses `format: email` fields from it; our `owner_email`/`email` should be marked `format: email`.
- Keep attribute *keys* stable (they're the operator targets — 2.4) and document them.

### 2.4 Search operators — GAP
Current: none declared.

**Implement** `search_operators` (manifest → connector-manager → Redis `search:operators` → searcher resolves `op:value`):

| operator | attribute_key | value_type |
|---|---|---|
| `owner` | `owner_email` | person |
| `status` | `case_status`/`lead_status`/`task_status` (object-aware values in attrs) | text |
| `priority` | `priority` | text |
| `stage` | `stage` | text |
| `account` | `account_name` | text |
| `industry` | `industry` | text |
| `type` | `account_type`/`case_type` | text |
| `lead_source` | `lead_source` | text |
| `created` | `created_date` | datetime |
| `modified` | `modified_date` | datetime |

(Filter to the set actually emitted — final list in implementation.)
Also add `in:salesforce` alias in searcher `resolve_source_alias` (Rust, + unit test).

### 2.5 Permissions — GAP (critical, currently `public=True` which is wrong for a CRM)
Salesforce sharing model to mirror:
1. **Owners**: every record's `OwnerId` (user or queue) → document `permissions.users=[owner_email]` (resolve via users map; queue-owned → groups).
2. **Public groups & queues**: `Group` (Type in Public/Queue) + `GroupMember` → `ctx.emit_group_membership(group_email, member_emails)`. Groups table is keyed `(source_id, email)` — Salesforce groups have no email, so synthesize a **stable deterministic email** per group: `sfgroup:{GroupId}@salesforce.local` (also used in doc permissions).
3. **Role hierarchy**: `UserRole` + hierarchy → emit each role as a group whose members are the role's members **plus all members of descendant roles** (so a manager searching sees subordinates' records, mirroring the role hierarchy). Document in plan: roles as groups only where records are role-hierarchy-scoped.
4. **Manual shares**: query `{Object}Share` records with `RowCause != 'Owner'` (AccountShare, ContactShare, OpportunityShare, LeadShare, CaseShare) → `UserOrGroupId` resolves to user or group → add to record's `permissions.users`/`groups`.
5. **Org-wide defaults**: config-driven per-object `public_read: true/false` (admin sets it in source config / connector defaults conservative). Only then `public=True`.
6. **Users**: sync active `User` records → `ctx.emit_person_sync(PersonSyncRecord)` (people directory, migration 109 feature) and keep the owner-email map in memory during sync. Users with deactivated → `emit_person_deleted`.
7. Queue membership: queues are `Group` Type='Queue' — same group path (records owned by a queue get `permissions.groups=[queue]`).

Emit order per object: groups/users/shares **before** records so permission resolution has the maps; emit all group events before the checkpoint that covers records.

### 2.6 Actions — GAP
Current: none. `execute_action` override absent → 404 for everything.

**Implement** (darwinbox/clickup patterns; `ActionDefinition` with `input_schema`, `mode`, `source_types=["salesforce"]`, `admin_only`):
- read: `get_case` (case by id/status/priority), `find_record` (SOQL search on a type)
- write: `create_case`, `update_case_status`, `create_task`, `update_task_status`, `log_call`? (final list in implementation — keep small, high-value, all realizable via `simple-salesforce` CRUD)
- `execute_action(action, params, credentials, source, actor_email)` → `ActionResponse.success/failure` with proper status codes; auth failure → 401; Salesforce API error → mapped 4xx/502 (clickup pattern). Override `execute_action` with concrete typed params.

---

## 3. Typing — eliminate `dict[str, Any]`

Follow the clickup connector's typing discipline (`Mapping[str, object]` at the wire, dataclasses everywhere else):

- `models.py` (new): typed dataclasses —
  - `SalesforceSourceConfig` (`from_mapping`, typed fields: `instance_url | None`, `objects: set[str]`, `public_read_objects: set[str]`, `realtime_poll_seconds`, `sync_users`, `sync_groups`, `sync_shares`)
  - `SalesforceRecord` (Id, SystemModstamp, fields dict typed via per-object dataclasses: `AccountRecord`, `ContactRecord`, `OpportunityRecord`, `LeadRecord`, `CaseRecord`, `TaskRecord`, `UserRecord`, `GroupRecord`, `GroupMemberRecord`, `RoleRecord`, `ShareRecord`)
  - `SalesforceCheckpoint`, `ObjectProgress`, phase enums (clickup-style with `from_mapping`/`to_json`, versioned)
  - `SalesforceObjectConfig` (replaces `dict[str, dict[str, list[str]]]` in config.py)
- `config.py`: typed constants (`list[SalesforceObjectConfig]`), object/field/schema definitions.
- `client.py`: keep `simple_salesforce` at the boundary but **parse every response into the typed models immediately** (`parse_account(record: Mapping[str, object]) -> AccountRecord`, fail loudly on shape mismatch); add `get_updated`, `get_deleted`, `query_users`, `query_groups`, `query_group_members`, `query_roles`, `query_shares`.
- `connector.py` / `sync.py`: no `dict[str, Any]` in signatures — `Mapping[str, object]` for the wire params (matching SDK base class), typed everything else. `sync()` signature must match the SDK base class exactly.
- `actions.py`: typed `ActionParams` dataclasses per action.
- `mappers.py`: `map_*_to_document(record: AccountRecord, ...) -> Document`; attributes built from typed fields.
- Keep `Any` only where the SDK itself forces it (e.g. `attributes: dict[str, Any] | None` on `Document` — boundary only, built from typed data).

---

## 4. File-by-file change list

### Connector (rewrite in place)
- `connectors/salesforce/salesforce_connector/__init__.py` — export new classes
- `.../models.py` — **new**: typed config/checkpoint/records (above)
- `.../config.py` — typed object configs + public-read defaults + API version + realtime defaults
- `.../client.py` — typed parsing, updated/deleted REST, users/groups/roles/shares queries
- `.../connector.py` — source_types, sync_modes full+incremental+realtime, checkpointing, watermark invalidation via connector_state, group/person events, permissions, actions wiring, search operators, manifest extras
- `.../mappers.py` — typed record → Document (content, metadata, attributes, permissions)
- `.../pagination.py` — keyset pagination for delta queries
- `.../permissions.py` — **new**: role-tree → groups, share resolution, group email synthesis
- `.../actions.py` — **new**: action definitions + typed execution
- `pyproject.toml` — deps: `omni-connector` (match master's python version), `simple-salesforce`; dev deps match hubspot/clickup on master
- `Dockerfile`, `main.py` — already match master's hubspot pattern; keep

### Tests (rewrite against current harness)
- `tests/conftest.py` — mock server: SOQL query endpoint + pagination, `/deleted`, `/updated`, User/Group/GroupMember/UserRole/{Object}Share fixtures, record fixtures with owners/queues/shares
- `test_full_sync.py` — full sync emits docs with permissions (owner user/group, public-read object), groups + memberships, person_sync events
- `test_incremental_sync.py` — **new**: watermark advance, delta emit, deleted tombstones, overlap window
- `test_checkpointing.py` — **new**: mid-sync checkpoint + resume (keyset continuation, no duplicate emits), watermark invalidation on field-set change (connector_state)
- `test_realtime_sync.py` — **new**: polling loop emits created/updated/deleted within poll window
- `test_actions.py` — **new**: action dispatch, auth failure 401, success path
- `test_mappers.py`, `test_auth_failure.py` — update for typed models

### Rust / shared
- `shared/src/models.rs` — `SourceType::Salesforce` + `as_str` + `ServiceProvider::Salesforce`; fix compiler-flagged exhaustive matches
- `services/migrations/112_add_salesforce_source_type.sql` — **new**, windshift-108 constraint shape + `salesforce`
- `services/connector-manager/src/config.rs` — `SALESFORCE_CONNECTOR_URL`
- `services/searcher/src/query_parser.rs` — `in:salesforce` alias + test
- (compiler will surface any other exhaustive `SourceType` matches — fix them)

### Web
- `web/src/lib/types.ts` — SALESFORCE in `SourceType`/`ServiceProvider` (rebased), `DEFAULT_SYNC_INTERVAL_SECONDS[SourceType.SALESFORCE]`
- `web/src/lib/utils/icons.ts` — icon mapping (rebased)
- `web/src/lib/utils/sources.ts` — `getSourceNoun` → `records`
- `web/src/routes/(admin)/admin/settings/integrations/+page.server.ts` / `+page.svelte` — merge staged hunks onto master
- `web/src/lib/components/salesforce-connector-setup.svelte` — align with master's hubspot setup component (API shape)
- `web/src/routes/(admin)/admin/settings/integrations/salesforce/[sourceId]/+page.server.ts` / `+page.svelte` — rewrite to master's hubspot page pattern
- `web/src/lib/images/icons/salesforce.svg` — keep

### Docker / env
- `.env.example` — rebase staged additions (ports, URLs, ENABLED_CONNECTORS list)
- `docker/docker-compose.yml`, `docker/docker-compose.dev.yml` — rebase staged service blocks onto master

---

## 5. Verification

1. `cargo build` (workspace) — catches all exhaustive `SourceType` matches
2. `cargo test -p omni-searcher` (query_parser `in:` alias test)
3. `uv run pytest connectors/salesforce/tests` — integration suite against real Postgres/Redis/connector-manager via `OmniTestHarness`
4. `ruff check` + `mypy --strict` on the connector (per pyproject)
5. Manual smoke: register connector → manifest shows sync_modes/operators/actions; source sync full → docs in index with permissions; edit a Salesforce record → incremental picks it up; delete → tombstone; change `config.py` field-set → connector_state invalidates → full resync
6. `docker compose -f docker/docker-compose.yml -f docker/docker-compose.dev.yml --env-file .env up -d` with `ENABLED_CONNECTORS=salesforce`

---

## 6. Sequencing

1. Rust/shared/migration + searcher alias (unblocks everything, small)
2. Connector core: models/config/client/pagination/mappers
3. Sync engine: full + incremental + checkpointing + watermark invalidation
4. Permissions: users/groups/roles/shares + person/group events
5. Realtime polling sync
6. Actions + search operators + manifest
7. Web + docker rebases
8. Tests (write alongside 3–6), lint/typecheck, full verification

## 8. Implementation status (2026-08-18)

**Done and verified:**
- Rust: `SourceType::Salesforce` + `as_str`/`TryFrom` + `ServiceProvider::Salesforce`; workspace builds; searcher `in:salesforce`/`in:sf` aliases + tests (`cargo test` green). `config.rs` needs no change (connector URLs are now self-registered via the SDK manifest).
- Migration `112_add_salesforce_source_type.sql` (windshift-108 shape + salesforce).
- Connector rewritten: `models.py` (typed records/config/checkpoint), `client.py` (typed, restful-based, honors http/https instance URLs), `pagination.py` (keyset full/delta SOQL), `mappers.py`, `permissions.py` (owner/queue/role-hierarchy/share resolution, group/role email synthesis), `actions.py` (6 actions with typed params), `connector.py` (full + incremental + realtime, per-object keyset cursors, watermark invalidation via schema fingerprint in connector_state, person + group events, search operators). `ruff` and `mypy --strict` clean.
- Tests: 23 unit + 7 action/HTTP tests pass locally. Integration tests written (full/incremental/checkpointing/auth) against the shared `OmniTestHarness` mock server with SOQL-aware filtering.
- Web: `types.ts` (+enum, +sync-interval), `icons.ts`, `sources.ts`, integrations `+page.server.ts`/`+page.svelte`, `salesforce/[sourceId]` pages, `salesforce-connector-setup.svelte` (instance URL + access token). Docker: main + dev compose blocks, `.env.example`, CI build job + matrix entry.

**Not run here (environment limitation):** the full integration suite (full/incremental/checkpointing) needs the shared test harness which reaches containers via host-published ports; this sandbox's Docker port forwarding is broken (published ports refused from host; bridge IPs reachable), so the harness cannot start. The tests are written against the same harness hubspot uses and will run in CI/on a host with working Docker. Unit + action tests (which need no container networking) pass.

**Note:** `extra_schema`/`attributes_schema` were intentionally not declared (optional passthrough; peer Python connectors like hubspot/clickup don't declare them either).
