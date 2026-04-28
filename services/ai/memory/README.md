# Memory

The AI service embeds [mem0ai](https://github.com/mem0ai/mem0) in-process to provide long-term memory for chat and background agents.

## Why this exists

Memory allows chat and agents to persist context across sessions. It is treated as **non-critical infrastructure**: if memory fails to initialize or a call fails, the AI service degrades gracefully to no-memory behaviour. Features like chat and agents must never break entirely due to a memory failure.

## Components

The memory implementation lives in `services/ai/memory/`:

```
services/ai/memory/
├── bootstrap.py      # Builds mem0 configuration from AI service state (LLM, embedder)
├── mode.py           # Resolves the effective memory mode (off / chat / full)
├── role_bootstrap.py # Bootstraps the restricted mem0ai Postgres role
└── service.py        # Direct async wrapper around mem0.Memory, mimicking the old HTTP client
```

### `service.py`

This provides the core `MemoryService` interface, replacing the old external HTTP memory service. It exposes asynchronous methods that internally dispatch to mem0 via `fastapi.concurrency.run_in_threadpool` because mem0 is synchronous. 

The API surface matches what the `routers/memory.py` proxy and other internal callers expect:
- `add()`: Add messages to memory
- `search()`: Vector-search memories matching a query
- `list()`: List every memory stored for a `user_id`
- `delete()`: Delete a single memory by ID
- `delete_all()`: Delete every memory for a `user_id`

`user_id` is an opaque scoping key used for namespacing.

### Memory Namespaces & Trust Levels

The AI service uses different key shapes (`user_id` equivalent) for different subjects:

| Namespace | Rendered as | Why |
| --------- | ----------- | --- |
| `<user_id>` (chat) | `<untrusted-memory>` fence with safety contract | Content is extracted from connector data (Slack, Gmail, etc.) — treated as attacker-controlled |
| `user:<uid>:agent:<id>` (personal agent) | Plain `## What I remember` bullet list | Derived from the agent's own instructions and run summaries — not user-controlled data |
| `org_agent:<agent_id>` (org agent) | Plain `## Agent memory (from prior runs)` bullet list | Same rationale as personal agent |

The fence in chat memory instructs the model to treat the content as observations only, not instructions. Agent memory has no fence because it originates from admin-controlled instructions and the agent's prior run summaries.

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
  ├── agents/executor.py ── search + add for agent runs
  └── memory/
        ├── service.py   ── in-process mem0 wrapper
        ├── bootstrap.py ── mem0 configuration logic
        └── mode.py      ── resolve effective mode
  │
  ▼
Postgres + pgvector (shared main omni DB, accessed via mem0ai role)
```

### AI service proxy (`routers/memory.py`)

Browsers and the web backend talk to the AI service's `/memories` router, which enforces ownership and authorization before forwarding calls to the in-process `MemoryService`:

| Method | Path                                | Auth  | Purpose                                                                      |
| ------ | ----------------------------------- | ----- | ---------------------------------------------------------------------------- |
| GET    | `/memories`                       | user  | List caller's memories (scoped to `x-user-id`)                             |
| DELETE | `/memories`                       | user  | Delete all of caller's memories                                              |
| DELETE | `/memories/org-agent/{agent_id}`  | admin | Purge `org_agent:<id>` namespace — called on org agent delete             |
| DELETE | `/memories/user-agent/{agent_id}` | admin | Purge `user:<uid>:agent:<id>` namespace — called on personal agent delete |
| POST   | `/memories/agent/{agent_id}/seed` | admin | Seed agent memory from instructions — called on agent create/update         |
| DELETE | `/memories/{memory_id}`           | user  | Delete a single memory (ownership verified first)                            |

### Mode gate

Before ever calling the `MemoryService`, callers evaluate `memory.mode.resolve_memory_mode(user_mode, org_default)`:

- `org_default` is a **ceiling**, not a fallback. Users cannot exceed the organisation-wide setting the admin picked.
- `off` disables memory entirely for that request.
- `chat` enables memory for chat only.
- `full` additionally enables agent-run memory.

If the effective mode is `off`, the service is bypassed completely.

## Agent memory use-cases (UC-A and UC-B)

Background agents use memory in `full` mode only.

### UC-A — Personal agent (agent_type = "user")

A scheduled agent owned by a specific user. Its memory is **isolated** from the owner's chat memory.

| Step                            | What happens                                                                                                                                                                                            |
| ------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **Gate**                  | If `effective_mode != 'full'` — skip memory entirely                                                                                                                                                 |
| **Key**                   | `user:<agent.user_id>:agent:<agent.id>`                                                                                                                                                               |
| **Read** (before run)     | `search(query=agent.instructions, user_id=key, limit=5)` — retrieves the most relevant summaries from prior runs and injects them into the system prompt under `## Agent memory (from prior runs)` |
| **Write** (after summary) | `add([{role:"user", "content":"Agent task: {instructions}"}, {role:"assistant", "content":"Agent run summary: {summary}"}], user_id=key)` — fire-and-forget, non-blocking                                |

### UC-B — Org agent (agent_type = "org")

An organisation-wide agent that runs with access to all data, not scoped to any individual user.

| Step                            | What happens                                                            |
| ------------------------------- | ----------------------------------------------------------------------- |
| **Gate**                  | If `effective_mode != 'full'` — skip memory entirely                 |
| **Key**                   | `org_agent:<agent.id>`                                                |
| **Read** (before run)     | Same as UC-A: `search(query=agent.instructions, user_id=key, limit=5)` |
| **Write** (after summary) | Same turn structure as UC-A, fire-and-forget                            |

## Configuration

Runtime variables related to memory:

| Var | Purpose |
| --- | --- |
| `MEMORY_ENABLED` | Set to `true` (default) or `false` to toggle memory feature entirely |
| `MEM0AI_DATABASE_USER` | Postgres role name for mem0 (default `mem0ai`) |
| `MEM0AI_DATABASE_ROLE_PASSWORD` | Password for the mem0 role. Required if memory is enabled |
| `MEM0_HISTORY_DB_PATH` | SQLite history path (default `/tmp/mem0_history.db`) |

The chosen LLM and embedder configurations are inherited dynamically at startup from the admin settings available to the AI service. Supported providers:
- **LLM**: openai, openai_compatible, anthropic, gemini, bedrock, aws_bedrock
- **Embedder**: openai, local (TEI), jina, bedrock

## Database Isolation (`mem0ai` role)

Mem0 data is stored directly in the **main Omni Postgres database** (not a separate `omni_mem0` DB), but inside its own auto-created `mem0_memories_<fp>` tables in the `public` schema.

Isolation against the main Omni application data is enforced via a dedicated Postgres role: `mem0ai` (bootstrapped by `memory/role_bootstrap.py`).
- The `mem0ai` role has `CREATE` privileges on `public` (to bootstrap its mem0 tables).
- We explicitly `REVOKE ALL` on all standard Omni tables, sequences, and functions from the `mem0ai` role.
- We run `ALTER DEFAULT PRIVILEGES` to ensure future migrations don't accidentally leak table access to the `mem0ai` role.

This ensures mem0 functionality cannot access main application data, even in case of a prompt injection or bug.

## Switching embedding models

Each embedder configuration (provider + model + dimensions) produces a fingerprinted collection name (`mem0_memories_<sha256-12>`).

Switching embedders automatically creates a fresh collection — no data migration occurs and there are no dimension-mismatch crashes. The previous collection remains in Postgres but is no longer queried until the old embedder is restored.

`DELETE /memories?user_id=…` purges the user from **all** `mem0_memories*` tables, including stale ones from previous embedder configs.

## Operational notes

- **Storage**: vectors live in Postgres (pgvector) in the main Omni DB, but queried only via the `mem0ai` role.
- **History DB**: mem0 uses an SQLite event history. Path is configurable via `MEM0_HISTORY_DB_PATH` (defaults to `/tmp/mem0_history.db`). Compose mounts the `mem0_history` named volume at `/var/lib/mem0` so the file survives container restarts; vectors are in Postgres regardless.
- **Messages ring-buffer**: mem0 keeps the last 10 raw conversation messages in the SQLite history. Our `DELETE` call clears this buffer.
- **Failure mode**: In-process calls use exception swallowing. If the DB is down or mem0 is misconfigured, callers degrade to "no memories" smoothly without terminating the process.

## Development & Testing

Unit tests for memory components (`test_memory_role_bootstrap.py`, `test_memory_bootstrap.py`, `test_memory_service.py`) live in `services/ai/tests/unit/`. Since the new architecture is in-process, tests rely extensively on mocking (`unittest.mock`) to patch `mem0.Memory`, `psycopg`, and `sqlite3` driver behaviour, requiring no runtime LLM or real Postgres DB.
