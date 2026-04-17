# Memory service

A thin self-hosted wrapper around [mem0ai](https://github.com/mem0ai/mem0)
that provides long-term memory for chat and background agents. It runs as
its own container and is only reachable from the internal
Docker network — browsers never talk to it directly.

## Why this exists

mem0ai 2.x dropped the bundled REST server, so we ship our own minimal
FastAPI wrapper. It exists so the AI service has a single, stable URL to
post chat turns, search relevant memories, and let users inspect/delete
what is stored about them — without coupling to mem0's Python API.

The service is treated as **non-critical infrastructure**: if it is slow
or down, every caller degrades to no-memory behaviour. Callers must never
let a memory failure break a user-facing flow.

## Components

```
services/memory/
├── Dockerfile        # Python 3.12 + mem0ai + pgvector driver
├── entrypoint.sh     # Pulls LLM/embedder config from AI service at startup
├── server.py         # FastAPI REST surface over mem0
└── README.md         # This file
```

### `server.py`

Exposes these endpoints (all synchronous under the hood — mem0 calls are
dispatched via `run_in_threadpool`):

| Method | Path                     | Purpose                                                                     |
| ------ | ------------------------ | --------------------------------------------------------------------------- |
| GET    | `/health`              | Readiness probe — checks Postgres connectivity, returns 503 if unreachable |
| POST   | `/memories`            | Add a conversation turn for a `user_id`                                   |
| GET    | `/memories?user_id=…` | List every memory stored for a `user_id`                                  |
| POST   | `/search`              | Vector-search for memories matching a query                                 |
| DELETE | `/memories/{id}`       | Delete a single memory by its mem0 id                                       |
| DELETE | `/memories?user_id=…` | Delete every memory for a `user_id`                                       |

`user_id` is an opaque scoping key chosen by the caller. The AI service
uses different key shapes for different subjects:

- `<user_id>` — chat memory for a human user
- `user:<user_id>:agent:<agent_id>` — personal agent memory (isolated
  from the owner's chat memory, per design — see
  `docs/superpowers/specs/2026-04-16-memory-integration-design.md`)
- `org_agent:<agent_id>` — organisation agent memory (no human in scope)

The service trusts this key as-is. Ownership/authorisation is enforced
one layer up, in the AI service's `/memories` proxy router.

### `entrypoint.sh`

Before starting `uvicorn`, the entrypoint:

1. Polls the AI service (`/internal/memory/llm-config`) until it returns
   the provider config. This indirection avoids duplicating LLM/embedder
   credentials across containers — the AI service is the single source
   of truth.
2. Builds a `/tmp/mem0_config.json` that:
   - Points mem0's vector store at the shared Postgres with a
     per-embedder collection name (`mem0_memories_<sha256-12>` of
     provider+model+dims). Switching embedders creates a fresh
     collection instead of crashing on dimension mismatch; switching
     back restores the previous collection.
   - Propagates `embedding_dims` into the pgvector config so the
     collection table is created with the correct dimension.
3. Writes `history_db_path` to `/tmp/mem0_history.db` (ephemeral).
4. Execs `uvicorn server:app --host 0.0.0.0 --port 8888`.

### `Dockerfile`

Minimal Python 3.12 image with mem0ai, FastAPI/uvicorn, the pgvector
Python driver (`psycopg[binary]`), and `httpx` (used by the entrypoint
to probe the AI service). `curl` is installed for health checks.

## How it's wired into the rest of Omni

```
browser
  │
  ▼
web (SvelteKit)
  │  x-user-id: <session user>
  ▼
ai service (:3003)
  ├── routers/chat.py    ── search + add for chat turns
  ├── routers/memory.py  ── session-scoped proxy (list/delete)
  ├── routers/internal.py── /internal/memory/llm-config (bootstrap)
  ├── agents/executor.py ── search + add for agent runs
  └── memory/
        ├── client.py    ── async httpx wrapper (this is the caller)
        └── mode.py      ── resolve effective mode (org ceiling / user)
  │
  ▼  http://memory:8888
memory service (this folder)
  │
  ▼
Postgres + pgvector (shared DB)
```

The AI service's `memory/client.py` is the only caller. It is deliberately
thin and best-effort: `search`/`add` swallow errors and log warnings;
`list`/`delete`/`delete_all` return booleans so the UI can reflect
success. This folder (the sidecar) implements the server side of that
same surface.

### Mode gate

Before ever calling this service, the AI service evaluates
`memory.mode.resolve_memory_mode(user_mode, org_default)`:

- `org_default` is a **ceiling**, not a fallback. Users cannot exceed
  the organisation-wide setting the admin picked.
- `off` disables memory entirely for that request.
- `chat` enables memory for chat only.
- `full` additionally enables agent-run memory.

If the effective mode is `off`, no request reaches this service.

### Stored memories vs. all collections

`GET /memories?user_id=` lists memories from the **active** embedder's
collection only. This is intentional — search and add also use the active
collection, so only those memories are actionable.

`DELETE /memories?user_id=` purges the user's rows from **every**
`mem0_memories*` table (active + any leftover tables from previous
embedder configs). This ensures "Delete all" in the UI is complete even
if the admin has ever switched embedders.

## Agent memory use-cases (UC-A and UC-B)

Background agents use memory in `full` mode only. The caller is
`services/ai/agents/executor.py`; everything below describes what it
does just before and just after each agent run.

### UC-A — Personal agent (agent_type = "user")

A scheduled agent owned by a specific user. Its memory is **isolated**
from the owner's chat memory so that agent-run context does not pollute
the user's conversational recall.

```
effective_mode = resolve_memory_mode(agent.owner.memory_mode, org_default)
                 ──────────────────────────────────────────────────────────
                 org_default is the ceiling; owner's setting is honoured
                 as long as it does not exceed the ceiling.
```

| Step                            | What happens                                                                                                                                                                                            |
| ------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **Gate**                  | If `effective_mode != 'full'` — skip memory entirely                                                                                                                                                 |
| **Key**                   | `user:<agent.user_id>:agent:<agent.id>`                                                                                                                                                               |
| **Read** (before run)     | `search(query=agent.instructions, user_id=key, limit=5)` — retrieves the most relevant summaries from prior runs and injects them into the system prompt under `## Agent memory (from prior runs)` |
| **Write** (after summary) | `add([{role:"user", content:"Agent task: {instructions}"}, {role:"assistant", content:"Agent run summary: {summary}"}], user_id=key)` — fire-and-forget, non-blocking                                |

**Why `agent.instructions` as the search query?**
Chat uses the last user message as the search anchor because that is the
live semantic intent. Agents have no live user turn — they are triggered
on a schedule. `agent.instructions` is the stable semantic description of
what the agent does, making it the closest equivalent anchor. Across many
runs, mem0 distils recurring outcomes into a compact fact list that fits
in the system prompt without growing unboundedly.

**Why a separate key namespace from chat?**
The owner's chat memory (`user_id = user.id`) reflects personal
conversation context. Agent runs are task-execution records. Mixing them
would pollute conversational recall with operational summaries and vice
versa.

### UC-B — Org agent (agent_type = "org")

An organisation-wide agent that runs with access to all data, not scoped
to any individual user.

```
effective_mode = org_default if org_default is valid else 'off'
                 ────────────────────────────────────────────────
                 No per-user setting applies — org default is both
                 the ceiling and the floor.
```

| Step                            | What happens                                                            |
| ------------------------------- | ----------------------------------------------------------------------- |
| **Gate**                  | If `effective_mode != 'full'` — skip memory entirely                 |
| **Key**                   | `org_agent:<agent.id>`                                                |
| **Read** (before run)     | Same as UC-A:`search(query=agent.instructions, user_id=key, limit=5)` |
| **Write** (after summary) | Same turn structure as UC-A, fire-and-forget                            |

Org agent memory is shared across every trigger of that agent, so mem0
accumulates institutional knowledge about what the agent has done over
time — independent of who triggered it.

## Configuration

Runtime env vars (set via Docker Compose):

| Var                             | Purpose                                                               |
| ------------------------------- | --------------------------------------------------------------------- |
| `AI_SERVICE_URL`              | Where to fetch the merged LLM/embedder config                         |
| `DATABASE_HOST`               | Postgres host (shared with the rest of Omni)                          |
| `DATABASE_PORT`               | Postgres port (default 5432)                                          |
| `DATABASE_NAME`               | Main app Postgres db name (used to bootstrap the mem0 db)             |
| `DATABASE_USERNAME`           | Postgres user (must have CREATEDB or the mem0 db pre-created)         |
| `DATABASE_PASSWORD`           | Postgres password                                                     |
| `MEMORY_DATABASE_NAME`        | Name of the isolated mem0 database (default:`<DATABASE_NAME>_mem0`) |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | OTLP collector endpoint; leave empty for local-only tracing           |
| `OTEL_DEPLOYMENT_ENVIRONMENT` | Environment label in traces (default:`development`)                 |
| `SERVICE_VERSION`             | Version tag in traces (default:`0.1.0`)                             |

The LLM/embedder provider + model are **not** configured here — they
come from the AI service response so the memory sidecar always matches
whatever the admin has configured in `/admin/settings/llm`.

## Database isolation

The memory service writes to a **separate Postgres database** (`omni_mem0` by
default, `omni_mem0_dev` in dev). This keeps mem0's pgvector tables out of the
main app schema and avoids any interference with the omni-migrator.

The entrypoint auto-creates this database if it does not exist. The Postgres
user must have `CREATEDB` privilege for this. In the default Docker Compose
setup the user is created as a superuser by the official postgres image, so no
extra steps are needed.

If your deployment uses a restricted Postgres user, pre-create the database
before starting the container:

```sql
CREATE DATABASE omni_mem0;
GRANT ALL PRIVILEGES ON DATABASE omni_mem0 TO <DATABASE_USERNAME>;
```

Then set `MEMORY_DATABASE_NAME=omni_mem0` in your environment. The entrypoint
will detect the existing database and skip the `CREATE DATABASE` step.

> **Note:** Omni requires [ParadeDB](https://github.com/paradedb/paradedb) for
> BM25 search. This rules out managed database services (AWS RDS, Cloud SQL,
> Azure Database for PostgreSQL) for the **main** Postgres instance. The mem0
> database only needs standard pgvector and has no ParadeDB dependency, but in
> practice both databases live on the same Postgres instance.

## Switching embedding models

Each embedder configuration (provider + model + dimensions) produces a
fingerprinted collection name (`mem0_memories_<sha256-12>`). Switching
embedders creates a fresh collection automatically — no data migration, no
dimension-mismatch crash. The previous collection remains in Postgres but is
no longer queried.

`DELETE /memories?user_id=…` purges the user from **all** `mem0_memories*`
tables, including stale ones from previous embedder configs, so "Delete all"
in the UI is always complete.

## Observability

The service is instrumented with OpenTelemetry (FastAPI + httpx auto-instrumentation).
When `OTEL_EXPORTER_OTLP_ENDPOINT` is set, spans are exported to the configured
collector alongside all other Omni services. Without an endpoint, traces are
collected locally only.

The `/health` endpoint performs an active Postgres connection probe and returns
`503` with a `reason` field if the mem0 database is unreachable. This makes it
useful as a Docker/Kubernetes readiness check.

## Operational notes

- **Storage**: vectors live in Postgres (pgvector, `omni_mem0` database).
  Durable across container restarts.
- **History DB**: mem0's SQLite event history goes to `/tmp/mem0_history.db`
  and is intentionally ephemeral. Only the pgvector store is the durable
  source of truth.
- **Messages ring-buffer**: mem0 keeps the last 10 raw conversation messages
  in the SQLite history and feeds them into the LLM extraction prompt on the
  next `add()` call. `DELETE /memories?user_id=…` clears this buffer so
  deleted facts are not re-extracted on the next chat turn.
- **Start order**: the AI service must be up before this container can finish
  starting; the entrypoint retries up to 30× with 2 s backoff.
- **Failure mode**: callers treat memory failures as non-fatal. A dead sidecar
  means users get "no memories" on search and fire-and-forget writes quietly
  drop. No user-facing error is raised.

## Development & Testing

The service includes integration tests that verify the FastAPI endpoints and their interaction with the memory logic.

### Running tests

From the `services/memory` directory:

```bash
uv run pytest
```

### Testing strategy

- **Isolation**: Tests use `unittest.mock` to replace `mem0.Memory` and database drivers (`psycopg`, `sqlite3`). No real LLM or Postgres instance is required.
- **Lifespan**: The tests exercise the FastAPI `lifespan` handler by manually invoking it in the `client` fixture. This ensures module-level globals like `_memory` and `_db_config` are correctly initialized and patched before any test request is made.
- **Config**: A temporary test configuration is written to `/tmp/mem0_config.json` before tests start, matching the structure used by the production `entrypoint.sh`.
- **Cleanup**: The `DELETE /memories?user_id=…` tests verify the multi-collection purge logic by mocking the Postgres system catalogue.
