# Memory service

A thin, self-hosted FastAPI wrapper around [mem0ai](https://github.com/mem0ai/mem0)
that provides long-term memory for chat turns and background agent runs.
The service runs as its own Docker container on the internal network — the
browser never talks to it directly.

## Contents

- [Why this exists](#why-this-exists)
- [Directory layout](#directory-layout)
- [How it fits into Omni](#how-it-fits-into-omni)
- [HTTP API](#http-api)
- [Namespaces and trust levels](#namespaces-and-trust-levels)
- [Memory modes (the gate)](#memory-modes-the-gate)
- [AI-service memory proxy](#ai-service-memory-proxy)
- [Use cases](#use-cases)
- [Startup: `entrypoint.sh`](#startup-entrypointsh)
- [Database isolation and collection fingerprinting](#database-isolation-and-collection-fingerprinting)
- [Supported providers](#supported-providers)
- [Configuration (env vars)](#configuration-env-vars)
- [Observability](#observability)
- [Operational notes](#operational-notes)
- [Development and testing](#development-and-testing)

---

## Why this exists

mem0ai 2.x dropped its bundled REST server. We ship our own minimal FastAPI
wrapper so the AI service has a single stable URL to post chat turns,
search relevant memories, and let users inspect/delete what is stored
about them — without coupling to mem0's Python API.

The service is **non-critical infrastructure**: if it is slow or down,
every caller degrades to no-memory behaviour. Callers must never let a
memory failure break a user-facing flow.

## Directory layout

```
services/memory/
├── Dockerfile        # Multi-stage Python 3.12 build (builder + runtime)
├── entrypoint.sh     # Pulls LLM/embedder config from AI service, bootstraps DB
├── pyproject.toml    # mem0ai, fastapi, psycopg[binary,pool], httpx, OTEL
├── server.py         # FastAPI REST surface over mem0
├── tests/            # Integration tests (mem0 + psycopg mocked)
└── readme_v2.md      # This file
```

## How it fits into Omni

```
browser
  │
  ▼
web (SvelteKit, :3000)
  │  x-user-id: <session user>
  ▼
ai service (FastAPI, :3003)
  ├── routers/chat.py      ── search + add for chat turns
  ├── routers/memory.py    ── user/admin proxy (list, delete, seed)
  ├── routers/internal.py  ── /internal/memory/llm-config (startup bootstrap)
  ├── agents/executor.py   ── search + add for agent runs
  └── memory/
        ├── client.py      ── async httpx wrapper (only caller of :8888)
        └── mode.py        ── resolve_memory_mode(user_mode, org_default)
  │
  ▼  http://memory:8888
memory service (this folder, FastAPI + mem0)
  │
  ▼
Postgres + pgvector  (separate `omni_mem0` database on the shared instance)
```

The AI service's `memory/client.py` is the only caller of this sidecar.
`search`/`add` swallow errors and log warnings; `list`/`delete`/`delete_all`
return booleans so the UI can reflect success.

## HTTP API

All endpoints are synchronous under the hood — mem0 calls are dispatched
via `run_in_threadpool`. The `user_id` query/body field is an opaque
scoping key supplied by the caller; ownership/authorisation is enforced
one layer up in the AI service's proxy.

| Method | Path                     | Purpose                                                                            |
| ------ | ------------------------ | ---------------------------------------------------------------------------------- |
| GET    | `/health`                | Readiness probe: opens a Postgres connection to the mem0 DB. 503 if unreachable.   |
| POST   | `/memories`              | Extract facts from a conversation turn and add them for `user_id`.                 |
| GET    | `/memories?user_id=…`    | List every memory in the **active embedder's** collection for `user_id`.          |
| POST   | `/search`                | Vector-search the active collection for memories matching `query`.                 |
| DELETE | `/memories/{id}`         | Delete a single memory by its mem0 id.                                             |
| DELETE | `/memories?user_id=…`    | Delete every memory for `user_id` across **all** `mem0_memories*` tables.         |

### POST /memories — vision-message sanitisation

mem0's `parse_vision_messages` calls `get_image_description(..., llm=None)`
for **any** list-typed `content`, not just actual images — which crashes
when no vision LLM is configured. The server defensively flattens list
content to a text-only string before forwarding to mem0, keeping only
blocks with `type == "text"`. A message that becomes empty after
flattening is dropped; if the whole batch collapses, the endpoint
returns `{}` without calling mem0. The AI-service chat writer applies
the same text-only extraction before it calls `add()`.

### POST /search and GET /memories — mem0 v2 filters

mem0 v2 requires entity-ID scoping via `filters={"user_id": ...}`, not as
a top-level kwarg. The server wraps that detail so callers just pass
`user_id`. Results coming back as a bare list are wrapped in
`{"results": [...]}`; dict responses are returned as-is so future mem0
pagination keys are forwarded unchanged.

### DELETE /memories?user_id=… — multi-collection purge + history clear

`DELETE` is deliberately more aggressive than `GET`/`POST`:

1. `mem0.delete_all(user_id=…)` clears the active collection.
2. A SQLite cleanup clears the mem0 **messages ring-buffer**:
   `DELETE FROM messages WHERE session_scope = 'user_id=<id>'`. Without
   this step, deleted facts would be re-extracted on the next `add()`
   because mem0 feeds the last 10 raw messages back into the LLM
   extraction prompt.
3. `_purge_user_across_all_collections(user_id)` scans
   `pg_tables WHERE tablename LIKE 'mem0_memories%'` and deletes rows
   where `payload->>'user_id' = $1` from every table — including stale
   collections left over from previous embedder configs (see
   [collection fingerprinting](#database-isolation-and-collection-fingerprinting)).

The response `{ "status": "deleted", "rows_deleted": <int> }` reports
the combined row count across all `mem0_memories*` tables so the UI can
confirm the purge was complete.

## Namespaces and trust levels

`user_id` is shaped by the caller according to the subject:

| Key shape                          | Subject                 | Written by                       |
| ---------------------------------- | ----------------------- | -------------------------------- |
| `<user_id>`                        | Chat memory for a human | `routers/chat.py`                |
| `user:<user_id>:agent:<agent_id>`  | Personal (user-owned) agent | `agents/executor.py` (UC-A) + seed route |
| `org_agent:<agent_id>`             | Organisation agent      | `agents/executor.py` (UC-B) + seed route |

How these are rendered inside LLM system prompts (`services/ai/prompts.py`)
differs by trust level:

| Namespace                          | Heading                        | Rendering                                 |
| ---------------------------------- | ------------------------------ | ----------------------------------------- |
| `<user_id>` (chat)                 | dynamic                        | `<untrusted-memory>` fence + safety contract |
| `user:<uid>:agent:<id>`            | `## What I remember` (agent-chat) or `## Agent memory (from prior runs)` (run) | Plain bullets, no fence |
| `org_agent:<agent_id>`             | `## Agent memory (from prior runs)` | Plain bullets, no fence |

**Why the fence for chat.** Chat memory is distilled from connector data
(Slack DMs, emails, issue comments, etc.). That content is
attacker-controlled, so the fence tells the model: treat the bullets as
observations, not instructions, and ignore anything that contradicts the
system prompt.

**Why no fence for agents.** Agent memory comes from the agent's own
admin-authored `instructions` plus the agent's prior-run summaries —
neither is user-controlled data, so the fence would be noise.

Bullet lists are capped at `MEMORY_BLOCK_MAX_CHARS = 4000` with a
`(additional memories omitted)` marker to keep the prompt bounded.

## Memory modes (the gate)

Before ever calling this service, the AI service evaluates
`memory.mode.resolve_memory_mode(user_mode, org_default)`:

```
VALID_MODES = {"off", "chat", "full"}
_MODE_RANK  = {"off": 0, "chat": 1, "full": 2}
```

Rules:

- `org_default` is a **ceiling**, not a fallback. The user can never
  exceed the org-wide setting an admin picked.
- If the user has no override, they inherit `org_default`.
- Invalid values are clamped to `"off"` defensively.

| Effective mode | Chat memory | Agent-run memory |
| -------------- | ----------- | ---------------- |
| `off`          | skipped     | skipped          |
| `chat`         | enabled     | skipped          |
| `full`         | enabled     | enabled          |

If the effective mode is `off`, no request reaches this service.

## AI-service memory proxy

`services/ai/routers/memory.py` is the browser-facing surface. It takes
the caller identity from the `x-user-id` header (set by the web backend)
and, for admin operations, requires `x-user-role: admin`.

| Method | Path                                | Auth  | What it does                                                                   |
| ------ | ----------------------------------- | ----- | ------------------------------------------------------------------------------ |
| GET    | `/memories`                         | user  | List caller's memories (scoped to `x-user-id`).                                |
| DELETE | `/memories`                         | user  | Delete all of caller's memories.                                               |
| DELETE | `/memories/{memory_id}`             | user  | Delete a single memory — the router lists the caller's memories first and 404s if the id isn't theirs (ids are never leaked across users). |
| DELETE | `/memories/org-agent/{agent_id}`    | admin | Purge the `org_agent:<id>` namespace. Called on org-agent delete.              |
| DELETE | `/memories/user-agent/{agent_id}?owner_user_id=…` | admin | Purge `user:<owner>:agent:<id>`. Called on personal-agent delete.      |
| POST   | `/memories/agent/{agent_id}/seed`   | admin | Re-seed an agent namespace from its instructions (see below).                  |

### Agent memory seeding (`POST /memories/agent/{agent_id}/seed`)

Called by the web layer whenever an agent is created or its `name`,
`instructions`, `schedule_type`, or `schedule_value` changes. The body
carries `owner_user_id` (absent/`null` for org agents). The route:

1. Computes the namespace (`org_agent:<id>` or `user:<owner>:agent:<id>`).
2. Calls `client.delete_all(namespace)` so stale facts from the previous
   instructions don't persist.
3. Writes one seed turn:
   ```
   user:      "Agent task: {instructions}"
   assistant: "I am the '{name}' agent. My task: {instructions}.
              Schedule: {schedule_type} {schedule_value}."
   ```
   mem0 extracts facts from this turn into the agent's initial memory.
   Background runs layer run-summary memories on top using the same
   namespace and turn structure.

## Use cases

### Chat (modes `chat` or `full`)

`routers/chat.py` resolves `effective_mode` from the chat's user mode +
`memory_mode_default` configuration, then:

- **Read**: `search(query=last_user_message, user_id=chat.user_id, limit=5)`
  before building the system prompt. Results are injected under the
  untrusted-memory fence.
- **Write** (fire-and-forget): after the assistant turn completes, extract
  text-only content from the last user message and the final assistant
  message, then `add([{role:"user",...},{role:"assistant",...}], user_id=chat.user_id)`.
  Non-text blocks (images, tool results) are stripped before writing.

### UC-A — Personal agent (`agent_type == "user"`)

Scheduled agent owned by a specific user. Its memory is **isolated**
from the owner's chat memory so run summaries don't pollute
conversational recall.

```
effective_mode = resolve_memory_mode(agent.owner.memory_mode, org_default)
```

| Step                  | What happens                                                                                                    |
| --------------------- | --------------------------------------------------------------------------------------------------------------- |
| Gate                  | Skip unless `effective_mode == "full"`.                                                                         |
| Key                   | `user:<agent.user_id>:agent:<agent.id>`                                                                         |
| Read (pre-run)        | `search(query=agent.instructions, user_id=key, limit=5)` → injected under `## Agent memory (from prior runs)`. |
| Write (post-summary)  | `add([{role:"user", content:"Agent task: {instructions}"}, {role:"assistant", content:"Agent run summary: {summary}"}], user_id=key)` — fire-and-forget. |

**Why `agent.instructions` as the search anchor?** Chat uses the last
user message because that is the live semantic intent. Agents have no
live user turn (they are triggered on a schedule), so the stable
semantic description of what the agent does is the closest equivalent.
Across many runs mem0 distils recurring outcomes into a compact fact
list that fits in the system prompt without growing unboundedly.

### UC-B — Org agent (`agent_type == "org"`)

Organisation-wide agent, not scoped to any user.

```
effective_mode = org_default if org_default in VALID_MODES else "off"
```

| Step                  | What happens                                                        |
| --------------------- | ------------------------------------------------------------------- |
| Gate                  | Skip unless `effective_mode == "full"`.                             |
| Key                   | `org_agent:<agent.id>`                                              |
| Read (pre-run)        | Same as UC-A: `search(query=agent.instructions, user_id=key, limit=5)`. |
| Write (post-summary)  | Same turn structure as UC-A, fire-and-forget.                       |

Org-agent memory is shared across every trigger of that agent, so mem0
accumulates institutional knowledge about what the agent has done over
time — independent of who triggered it.

## Startup: `entrypoint.sh`

Before starting `uvicorn`, the entrypoint:

1. **Fetches config** from `${AI_SERVICE_URL}/internal/memory/llm-config`.
   Retries up to **30× with 2 s backoff**; exits non-zero if the AI
   service never responds. This indirection avoids duplicating LLM and
   embedder credentials across containers — the AI service is the single
   source of truth (admin settings on `/admin/settings/llm` and
   `/admin/settings/embeddings`).
2. **Ensures the mem0 database exists.** Resolves `MEMORY_DATABASE_NAME`
   (default `<DATABASE_NAME>_mem0`), connects to the main app DB,
   `CREATE DATABASE` if missing. Requires `CREATEDB` privilege, otherwise
   the entrypoint prints an instruction to create it manually and exits.
3. **Cleans up stale tables** in the main app database — any
   `mem0_memories*` tables left from earlier versions that wrote into the
   main DB are dropped so there is only one authoritative copy in
   `omni_mem0`.
4. **Fingerprints the collection name.** Takes
   `sha256("{provider}:{model}:{dims}")[:12]` over the embedder config
   and sets `collection_name = "mem0_memories_<fp>"`. Embedder changes
   automatically create a new collection — no dimension-mismatch crashes,
   no manual migration.
5. **Writes** `/tmp/mem0_config.json` with the merged vector-store + llm +
   embedder config and `history_db_path = /tmp/mem0_history.db`.
6. Execs `uvicorn server:app --host 0.0.0.0 --port 8888`.

At request time, `server.py`'s lifespan reads the same config file and
builds the `mem0.Memory` instance that backs every endpoint.

## Database isolation and collection fingerprinting

**Separate database.** mem0's pgvector tables live in their own Postgres
database (`omni_mem0` by default; `omni_mem0_dev` in dev). This keeps
them out of the main app schema and away from the `omni-migrator`. The
entrypoint auto-creates the database if the DB user has `CREATEDB`.
If your deployment uses a restricted Postgres user, pre-create it:

```sql
CREATE DATABASE omni_mem0;
GRANT ALL PRIVILEGES ON DATABASE omni_mem0 TO <DATABASE_USERNAME>;
```

Then set `MEMORY_DATABASE_NAME=omni_mem0` in the environment.

> **Note:** Omni's **main** Postgres requires [ParadeDB](https://github.com/paradedb/paradedb)
> for BM25 search, which rules out most managed services (RDS, Cloud SQL,
> Azure). The mem0 database only needs stock pgvector, but in practice
> both databases live on the same Postgres instance in every real
> deployment.

**Collection fingerprinting.** Each embedder config (provider + model +
dims) produces a collection name of the form `mem0_memories_<sha256-12>`.
Switching embedders creates a fresh collection — no migration. The old
collection stays in Postgres but is never queried. Switching back
reattaches to the previous collection.

**`GET` vs `DELETE` semantics.** `GET /memories` and `POST /search` hit
only the **active** collection (so the UI shows what is actionable).
`DELETE /memories?user_id=…` purges **every** `mem0_memories*` table so
"Delete all" is complete even after embedder switches.

## Supported providers

The memory sidecar follows whatever the AI service reports. Mapping is in
`services/ai/routers/internal.py`:

| Omni LLM `provider_type`   | mem0 `llm.provider`  |
| -------------------------- | -------------------- |
| `openai`, `openai_compatible` | `openai`          |
| `anthropic`                | `anthropic`          |
| `gemini`                   | `gemini`             |
| `bedrock`, `aws_bedrock`   | `aws_bedrock`        |

| Omni embedder `provider_type` | mem0 `embedder.provider` | Notes                                |
| ------------------------------ | ------------------------ | ------------------------------------ |
| `openai`                       | `openai`                 | Uses admin-configured API key / base |
| `local`                        | `openai`                 | TEI / OpenAI-compatible local        |
| `jina`                         | `openai`                 | OpenAI-compatible Jina endpoint      |
| `bedrock`                      | `aws_bedrock`            |                                      |

Any other provider causes `/internal/memory/llm-config` to return 503,
which fails the entrypoint startup with a clear error. mem0's OpenAI
embedder requires a non-empty API key even for local servers, so the
mapping sends `"unused"` when none is configured.

When the admin hasn't set `embedding_dims`, the AI service probes the
live provider by embedding a short test string and forwards the measured
dimension so pgvector's collection table is created with the correct
column width. mem0's own default (1536) would otherwise crash on many
local models.

## Configuration (env vars)

Set in Docker Compose (`docker/docker-compose.yml` +
`docker-compose.dev.yml`):

| Var                            | Purpose                                                               |
| ------------------------------ | --------------------------------------------------------------------- |
| `AI_SERVICE_URL`               | Where to fetch the merged LLM/embedder config at startup.             |
| `DATABASE_HOST`                | Postgres host (shared with the rest of Omni).                         |
| `DATABASE_PORT`                | Postgres port (default `5432`).                                       |
| `DATABASE_NAME`                | Main app DB name (used to bootstrap the mem0 DB).                     |
| `DATABASE_USERNAME`            | Postgres user. Needs `CREATEDB` unless mem0 DB is pre-created.        |
| `DATABASE_PASSWORD`            | Postgres password.                                                    |
| `MEMORY_DATABASE_NAME`         | Isolated mem0 DB name (default `<DATABASE_NAME>_mem0`).               |
| `OTEL_EXPORTER_OTLP_ENDPOINT`  | OTLP collector endpoint; empty = local-only spans.                    |
| `OTEL_DEPLOYMENT_ENVIRONMENT`  | Environment label in traces (default `development`).                  |
| `SERVICE_VERSION`              | Version tag in traces (default `0.1.0`).                              |

LLM/embedder provider+model are **not** configured here — they come from
the AI service's response, so the sidecar always matches what the admin
configured in `/admin/settings/llm` and `/admin/settings/embeddings`.

## Observability

Instrumented with OpenTelemetry: `FastAPIInstrumentor` (server spans) +
`HTTPXClientInstrumentor` (outgoing HTTP). When
`OTEL_EXPORTER_OTLP_ENDPOINT` is set, spans go to
`{endpoint}/v1/traces` via `OTLPSpanExporter`. Resource attributes:
`service.name=omni-memory`, `service.version=$SERVICE_VERSION`,
`deployment.environment=$OTEL_DEPLOYMENT_ENVIRONMENT`.

`GET /health` does an active Postgres connection open+close, returning
`503 {"status": "unhealthy", "reason": "..."}` on failure. This makes it
suitable as a Docker/Kubernetes readiness probe.

## Operational notes

- **Storage**: pgvector embeddings live in Postgres (`omni_mem0`
  database). Durable across container restarts.
- **History DB**: mem0's SQLite event history is written to
  `/tmp/mem0_history.db` and is intentionally ephemeral. Only the
  pgvector store is the durable source of truth.
- **Ring-buffer hygiene**: mem0 keeps the last 10 raw conversation
  messages in SQLite and feeds them into the LLM extraction prompt on
  the next `add()`. `DELETE /memories?user_id=…` clears this buffer so
  deleted facts are not re-extracted on the next turn.
- **Start order**: the AI service must be up before this container
  finishes starting; the entrypoint retries up to 30× with 2 s backoff.
- **Failure mode**: callers treat memory failures as non-fatal. A dead
  sidecar means users get "no memories" on search and fire-and-forget
  writes quietly drop — no user-facing error is raised.
- **Docker image**: multi-stage build on `python:3.12-slim`. The builder
  installs deps with `uv` (`UV_PROJECT_ENVIRONMENT=/usr/local`); the
  runtime stage copies just the site-packages + binaries + app files
  and installs only `curl` (used by the entrypoint health probe).

## Development and testing

The service includes integration tests for the FastAPI endpoints.

```bash
cd services/memory
uv sync
uv run pytest
```

### Test strategy

- **Isolation**: `unittest.mock.patch` replaces `mem0.Memory`, `psycopg`,
  and `sqlite3` — no real LLM or Postgres is needed.
- **Lifespan**: the `client` fixture enters the app's `lifespan` context
  manually so `server._memory` and `server._db_config` get initialised
  the same way production does.
- **Config**: `tests/test_service.py` writes a minimal
  `/tmp/mem0_config.json` at session start to match the shape
  `entrypoint.sh` produces.
- **Coverage**: health (ok + 503), add (empty / text / list-flattening /
  mixed), search (filters shape, top_k default, dict passthrough), list
  (wrapping vs passthrough), single delete, and delete-all
  (multi-collection purge + SQLite ring-buffer cleanup).

### Related code to check when changing this service

- `services/ai/memory/client.py` — the only HTTP caller.
- `services/ai/memory/mode.py` — mode ceiling logic.
- `services/ai/routers/memory.py` — browser-facing auth boundary.
- `services/ai/routers/internal.py` — `/internal/memory/llm-config` bootstrap.
- `services/ai/routers/chat.py` — chat read/write paths.
- `services/ai/agents/executor.py` — UC-A / UC-B read/write paths.
- `services/ai/prompts.py` — memory rendering (`<untrusted-memory>` fence
  vs. trusted bullets).
