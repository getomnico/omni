# Plan B: Build Native Agent Governance and Durable Workflows in Omni

## Status

Proposed for comparison. This is not an implementation commitment.

Before implementation, maintainers must confirm which governance and audit features belong in the community edition. Basic self-hosted runtime safety limits are likely community functionality; organization-wide policy administration and audit reporting may fall under Omni's commercial-edition guidance.

## Executive summary

Build BoundFlow-equivalent runtime governance, lifecycle policies, durable workflow execution, approval/input gates, audit records, and observability directly in Omni. Reuse Omni's existing durable `agent_runs` queue, leases, heartbeats, conversation WAL, provider abstractions, usage tracking, tool approvals, Postgres, Redis, and OpenTelemetry setup.

This approach keeps one control plane and one primary database, gives Omni complete product and API control, and allows incremental delivery. Reaching full BoundFlow feature parity is substantially more work than implementing runtime budgets alone, particularly for generalized workflow versioning, lifecycle policies, audit guarantees, and durable operation gates.

## Goals

Implement natively:

- Runtime policies:
  - Maximum LLM calls per run
  - Maximum cost per run
  - Maximum output tokens per call
  - Maximum model-call duration
  - Per-tool call limits
  - Per-tool failure limits
  - Policy-selected models
  - Typed Omni-specific limits
- Agent lifecycle policies based on recent-run metrics:
  - Model switching
  - LLM-call limit changes
  - Cost-limit changes
  - Token-limit changes
- Workflow lifecycle policies:
  - Pause
  - Cooldown
  - Workflow/agent-definition version rollback
- Durable multi-operation execution and resumption
- Durable approval and input gates
- Governance metrics and an append-only decision audit log
- OpenTelemetry-compatible operation, LLM, and tool traces
- Native policy administration APIs and UI

## Non-goals

- Creating a general-purpose third-party workflow platform unrelated to Omni agents
- Supporting arbitrary Python customer workers in the first release
- Replacing Omni's connector-specific OAuth or permission systems
- Guaranteeing that a cost cap stops a provider call before its final usage is known
- Exporting sensitive prompt/tool content by default
- Implementing organization-wide audit exports without maintainer approval

## Product scope

### Initial scope

- Background and scheduled agents
- Manual background-agent triggers
- All Omni model providers and tools
- Auxiliary calls for compaction and run summaries

### Later scope

- Interactive chats
- Reusable multi-step workflow definitions
- Connector-originated action workflows
- External worker SDK, only if a concrete ecosystem need emerges

The architecture should support later chat governance, but runtime-budget delivery should not wait for a generalized workflow engine.

## Existing Omni foundation

Reuse rather than replace:

- `services/ai/agents/scheduler.py`: schedule materialization
- `services/ai/agents/queue_worker.py`: durable claims, leases, and retries
- `services/ai/agents/executor.py`: model/tool loop
- `agent_runs`: durable run queue and result state
- `agent_run_logs`: durable conversation/action WAL
- `tool_approvals`: current approval records
- `model_usage`: provider-reported token accounting
- `ProviderCache`: model-to-provider credential resolution
- `ToolRegistry`: common tool-dispatch boundary
- Redis streams: interactive response delivery
- OpenTelemetry setup in `services/ai/telemetry.py`

The existing queue already provides much of the durable-execution substrate. Native work should evolve it rather than introducing a parallel scheduler.

## Target architecture

```text
Omni web / scheduler
        |
        v
Omni Postgres
  - agent definitions and versions
  - policies and lifecycle state
  - run/operation queue and WAL
  - approvals and input gates
  - metrics and audit events
        |
        v
omni-ai workflow scheduler / queue workers
        |
        v
Native RunGovernor --> Omni providers and tools
        |
        v
Native lifecycle evaluator --> next-run effective policy/state/version
        |
        v
OpenTelemetry + Omni admin UI/API
```

There is one durable state machine and one correlation ID: the Omni run ID.

## Service and table ownership

Preserve one writer per table:

| Tables | Writer |
|---|---|
| `agents`, `agent_versions` | `omni-web` until agent mutations move behind an AI API |
| Runtime/lifecycle policy tables | `omni-ai` through typed internal/admin APIs |
| Agent/workflow run and operation tables | `omni-ai` |
| Governance metrics and audit events | `omni-ai` |
| Approval/input decision tables | `omni-ai`; web submits through authenticated APIs |
| Model/provider configuration and pricing | Existing owning service, with AI as reader |

`omni-web` must not write governance tables directly. It should call authenticated `omni-ai` APIs and render their responses.

## Domain model

### Runtime policy

Define a concrete Pydantic model:

```python
class RuntimePolicy(BaseModel):
    max_llm_calls: int | None
    max_cost_usd: Decimal | None
    max_tokens_per_call: int | None
    max_call_seconds: float | None
    tool_call_limits: tuple[ToolCallLimit, ...]
    tool_failure_limits: tuple[ToolFailureLimit, ...]
    allowed_model_ids: tuple[str, ...]
    selected_model_id: str | None
    limits: OmniRuntimeLimits
```

Use `None` for no limit; do not overload zero to mean both blocked and unset.

Validate policies at write time:

- Non-negative values
- Referenced models are active
- Cost-limited models have pricing
- Tool names exist or are explicitly forward-declared
- A selected model is included in the allowed set
- User-agent policy cannot expand permissions granted by the agent definition

### Effective policy

At run creation, snapshot:

- Base runtime policy
- Applicable lifecycle-policy result
- Selected agent-definition version
- Model-pricing version
- Policy version/revision

A run never changes policy halfway through because an administrator edited the policy. Only usage accumulated within that snapshotted policy changes enforcement.

### Run governor

Add:

```text
services/ai/governance/
  __init__.py
  models.py
  governor.py
  pricing.py
  repository.py
  lifecycle.py
  audit.py
  tracing.py
```

`RunGovernor` owns:

- Call reservations
- Completed call count
- Input/output/cache tokens
- Estimated and actual cost
- Per-tool calls and failures
- Per-call timing
- Final termination reason
- Trace span construction

Use separate scoped governors for:

- `responder`
- `compactor`
- `run_summary`

A parent `WorkflowBudget` may impose a total cap across all scoped governors.

## Database design

Final names should follow a dedicated migration review, but the native design needs the following concepts.

### Model pricing

`model_pricing`:

- `id`
- `model_id`
- `input_per_million`
- `output_per_million`
- `cache_read_per_million`
- `cache_creation_per_million`
- `currency`
- `effective_from`
- `effective_until`
- `source`
- `created_at`

Historical runs reference or snapshot the pricing row used. Never recalculate old audit records using today's prices.

### Policies

`agent_runtime_policies`:

- `id`
- `agent_id`
- `revision`
- Typed policy JSON or normalized typed columns
- `created_by`
- `created_at`
- `superseded_at`

`agent_lifecycle_policies`:

- `id`
- `agent_id`
- `revision`
- Ordered typed rules
- `created_by`
- `created_at`

`workflow_lifecycle_policies`:

- `id`
- `workflow_definition_id`
- `revision`
- Ordered typed rules
- `created_by`
- `created_at`

Prefer typed application models around JSONB rules over a large collection of nullable action columns. Parse and validate JSONB at repository boundaries.

### Versioned definitions

`agent_versions`:

- Immutable snapshot of instructions, permissions, model defaults, tools, and execution-schema version
- Monotonic version per agent
- Creation actor and timestamp

The `agents` row points to the current requested version. Every run snapshots the effective version.

### Run governance snapshot and metrics

Extend `agent_runs` or add one-to-one tables for:

- Effective policy revision and snapshot
- Agent-definition version
- Pricing snapshot
- Effective model
- Total calls/tokens/cost
- Per-tool counts/failures
- Governance termination reason
- Lifecycle state at dispatch

Keep high-volume per-call metrics in a separate table if needed.

### Workflow operations

To reach complete durable-workflow parity, add:

`workflow_definitions`:

- Stable type/name and owner

`workflow_versions`:

- Immutable operation graph/schema and execution version

`workflow_instances`:

- Current version and lifecycle state
- Pause/cooldown/interrupted metadata

`workflow_runs`:

- Invocation, context, outcome, failure reason, timestamps

`workflow_operations`:

- Operation name, attempt, lease, timeout, input/output context, status

The existing `agent_runs` can initially act as the specialized single-operation workflow run. General tables should be introduced only when multi-operation gates are implemented.

### Approvals and inputs

Generalize the current approval model:

- `workflow_approvals`
- `workflow_inputs`

Store:

- Run and operation IDs
- Requested action/prompt
- Opaque metadata
- Opened/expiry/decision timestamps
- Actor
- Decision and reason
- Branch target

OAuth credentials and secrets remain in existing encrypted Omni stores.

### Governance audit

`governance_audit_events` is append-only:

- `id`
- `occurred_at`
- `actor_type`
- `actor_id`
- `event_type`
- `agent_id` / `workflow_id` / `run_id`
- Policy revision
- Rule and observed value
- Previous and resulting state
- Approval/input correlation
- Structured typed payload
- Optional integrity-chain fields

Updates and deletes should be prohibited through normal application repositories. Retention/export behavior requires separate product approval.

## Runtime-governance implementation

### Model calls

Refactor the provider-call boundary in `services/ai/agents/executor.py`:

1. Request a `ModelCallPermit` from `RunGovernor`.
2. Persist/report the worst-case reservation before provider invocation.
3. Resolve the policy-selected Omni model ID through `ProviderCache`.
4. Apply the smaller of request and policy output-token limits.
5. Wrap stream consumption in `asyncio.timeout` when configured.
6. Capture provider-reported usage.
7. Replace the reservation with actual usage.
8. Persist usage synchronously to the run ledger and asynchronously aggregate user-facing usage where safe.
9. Check whether another call is permitted before continuing the loop.

The current `UsageTracker` can supply the token values, but governance state must not depend solely on its fire-and-forget persistence.

### Cost reservations

Before a call, reserve a conservative maximum:

- Estimate input from the previous exact input plus growth allowance
- Use `max_tokens` for worst-case output
- Price cache categories independently where known

On completion, replace the reservation with exact provider-reported usage. On a confirmed pre-inference provider failure, release it. For ambiguous failures and timeouts, retain a conservative estimated charge or mark the charge uncertain rather than asserting that no billing occurred.

This is safer than assuming every provider exception is unbilled.

### Cost-cap semantics

Document explicitly:

- Exact cost is generally known only after a call
- A call may cross the cap
- The next call will be blocked
- Optional headroom can reduce overshoot
- Concurrent calls reserve independently
- Unknown pricing makes cost enforcement invalid and must fail before execution

### Tool calls

Wrap `ToolRegistry.execute(...)`:

- Authorize before execution
- Return a structured denied tool result without running the tool when the cap is spent
- Count only actual executions
- Record failures from both exceptions and structured `is_error` results
- Trip failure limits according to clear `>=` or `>` semantics
- Persist tool state before and after execution using the existing WAL

Tool-specific idempotency keys should include the run ID and tool-call ID.

### Auxiliary calls

Compaction and summary generation must not escape governance. Operators can set scoped policies plus a parent total-workflow budget.

If the main loop spends the total budget, prefer deriving a summary from durable run logs rather than making an unbudgeted final LLM call.

### Custom limits

Define typed Omni limits rather than an opaque dictionary where practical:

- Total downloaded bytes
- Sandbox executions and total runtime
- Search/fetch result count
- Connector reads and writes
- Number of distinct sources
- Total wall-clock duration

Enforce each at the owning boundary.

## Policy engine

### Runtime policy resolution

At run creation:

1. Load the latest base policy.
2. Load recent immutable run metrics.
3. Evaluate lifecycle rules in deterministic order.
4. Produce one typed effective policy.
5. Validate that it does not expand agent permissions.
6. Snapshot it on the run.
7. Append audit events for rules that changed effective behavior.

### Agent lifecycle metrics

Initially support:

- Tokens used
- Cost
- LLM calls
- Calls per tool
- Tool failures/failure rate
- Latency
- Run failures

Rules specify:

- Metric
- Comparison operator
- Threshold
- Recent-run window
- Optional tool
- Typed action

### Agent lifecycle actions

- Select configured model
- Set maximum LLM calls
- Set maximum cost
- Set maximum tokens per call
- Suspend an agent
- Apply cooldown

Actions affect future runs, never a run already in progress except through its snapshotted runtime governor.

### Workflow lifecycle actions

- Pause
- Cooldown until an absolute timestamp
- Set workflow/agent-definition version
- Mark interrupted and require operator resolution

Use optimistic concurrency or row locking so simultaneous run completions cannot apply contradictory transitions.

### Rule determinism

Define:

- Rule evaluation order
- Whether the first or all matching rules apply
- Conflict precedence
- Window behavior with fewer runs than requested
- Re-trigger behavior after resume
- Version boundaries for historical metrics

Persist the input metrics and selected rule result for every action.

## Durable workflow runtime

### Incremental approach

Do not build a generalized engine before runtime budgets ship.

1. Treat each current `agent_run` as a one-operation workflow.
2. Add durable blocked states for approval/input.
3. Refactor the executor into resumable operation segments.
4. Introduce generic workflow definition/operation tables only when multiple named operations are required.

### Operation state machine

Suggested states:

- `pending`
- `leased`
- `running`
- `awaiting_approval`
- `awaiting_input`
- `delayed`
- `completed`
- `failed`
- `interrupted`
- `abandoned`

Transitions occur in transactions and append an audit event. Leases and claim tokens fence stale workers, following the existing `agent_runs` pattern.

### Scheduling and concurrency

Reuse Postgres:

- `FOR UPDATE SKIP LOCKED`
- Claim tokens
- Lease expiry and heartbeats
- Advisory locks for per-agent serialization
- Configured queue depth
- Coalesce or FIFO invocation modes
- Delayed dispatch timestamps

Add partitioning only when measured scale requires it. A single Postgres queue should remain the default deployment.

### Resumption

The worker reconstructs execution from:

- Immutable run policy/version snapshot
- Durable operation context
- Agent conversation WAL
- Unanswered tool calls
- Approval/input decisions

A crash after an external side effect but before recording its result remains an at-least-once boundary. Tools must accept idempotency keys or implement reconciliation.

### Operation timeout

Store absolute operation deadlines. A scheduler/reconciler marks expired operations and cancels local tasks best-effort. A timeout is a customer-run failure unless infrastructure state is inconsistent, in which case mark the workflow interrupted.

## Approval and input gates

### Approval flow

1. Persist the requested tool call.
2. Create a pending approval and transition the operation atomically.
3. Notify clients.
4. Accept approve/reject with authenticated actor and optional reason.
5. Append an immutable decision event.
6. Queue the approved/rejected branch.
7. Resume from WAL without replaying prior completed calls.

No write tool may run before the approval transaction commits.

### Input flow

Implement parallel semantics for structured external input:

- Typed schema
- Prompt and safe metadata
- Timeout branch
- Actor identity
- Validation before resume

Never put provider credentials or OAuth tokens into workflow context.

### Existing tool approvals

Migrate or adapt `tool_approvals` rather than maintaining two unrelated approval systems. Existing chat approval behavior must remain compatible during rollout.

## Versioning and rollback

### Agent-definition versions

Every change to instructions, model defaults, allowed sources, allowed actions, or relevant execution settings creates an immutable version.

Runs snapshot a version. A lifecycle rollback updates the effective version pointer for future runs and records:

- Triggering rule
- Metric window
- Previous version
- Target version
- Actor (`system:policy-engine`)

### Workflow-handler versions

Code-level rollback is different from agent-definition rollback. Support it through explicit executor compatibility versions and deployment procedures. Do not claim database metadata can roll back unavailable application code.

### Metrics epochs

Lifecycle windows should not accidentally mix incompatible versions. Define whether a version switch starts a new metric epoch or whether rules opt into cross-version history.

## Observability

### Native spans

Emit an operation -> agent -> model/tool span hierarchy using OpenTelemetry GenAI conventions:

- Run ID as trace/correlation attribute
- Requested and effective model
- Token categories
- Cost and pricing revision
- Tool name/call ID
- Policy revision and termination reason
- Approval/input correlation

Prompt, response, and tool payload capture must be off or redacted by default.

### Metrics

Expose:

- Runs by status/outcome
- Cost/tokens/calls by agent/model/purpose
- Cap-exceeded counts
- Tool denial/failure counts
- Queue and operation latency
- Approval wait time
- Lifecycle action counts
- Interrupted workflows

### Audit

Provide typed retrieval APIs before building reports. Audit export and organization-wide reporting require maintainer confirmation.

## API design

Add authenticated `omni-ai` endpoints for:

- Runtime policy get/set/history
- Agent lifecycle policy get/set/history
- Workflow lifecycle policy get/set/history
- Effective policy preview
- Agent/workflow state
- Pause/resume/cooldown/interruption resolution
- Approval and input decisions
- Run metrics
- Governance audit events
- Model pricing management or validation

Use concrete request/response schemas. Validate all policy JSON at the API boundary.

## UI design

### Agent policy editor

- Runtime cap fields with clear unset semantics
- Tool-specific limits
- Allowed and fallback models
- Pricing-coverage warnings
- Effective policy preview

### Agent/run status

- Current lifecycle state
- Cooldown expiry
- Effective model/version
- Budget consumed
- Termination reason
- Approval/input cards

### Lifecycle rule editor

Deliver only after runtime policies are stable. Show deterministic rule order, recent metric values, and projected action.

### Audit view

If approved for the relevant edition, show time-ordered policy and approval decisions with actor, reason, input metric, and state transition.

All Svelte buttons must use `cursor-pointer` per project conventions.

## Deployment and operations

No new service or database is required for the initial design.

Changes include:

- Omni migrations
- `omni-ai` governance and workflow modules
- Existing scheduler/queue-worker extensions
- Web admin APIs/pages
- Optional CLI commands if required

Operational requirements:

- Migration rollback and compatibility plan
- Audit-table growth monitoring
- Pricing update process
- Lease/recovery dashboards
- Backup/restore tests covering policy, run, and audit consistency
- Feature flags for runtime enforcement, lifecycle evaluation, and generalized workflows

## Failure semantics

| Failure | Required behavior |
|---|---|
| Governance repository unavailable | Run does not start; no ungoverned fallback |
| Policy malformed in storage | Fail before execution and alert; boundary validation should prevent it |
| Selected model deleted | Fail before provider call or use an explicitly policy-approved fallback |
| Pricing missing with cost cap | Reject policy or fail before execution |
| Provider fails before billing | Release reservation only when confidently unbilled |
| Provider timeout/billing uncertain | Retain estimated charge and mark uncertainty |
| Tool cap spent | Return denial to the model without executing |
| Repeated tool failures | End run according to snapshotted policy |
| Worker crashes | Lease expires; next worker resumes from WAL |
| Approval service restarts | Pending gate remains durable |
| Conflicting lifecycle transitions | Transaction/optimistic revision allows one winner; loser reevaluates |
| Old code version unavailable | Refuse rollback and surface operator intervention |

## Implementation phases

### Phase B0: semantics and schema RFC

- Freeze policy types and unset/block semantics
- Define cost, failure, and lifecycle edge cases
- Define authority and table writers
- Review community/commercial placement

Exit criterion: approved domain model and migration strategy.

### Phase B1: native runtime governor

- Implement call/tool permits and counters
- Add per-call token limit and timeout
- Add per-run calls/tokens limits
- Persist termination reasons and metrics
- Integrate responder, compactor, and summary calls

Exit criterion: all current agent providers and tools pass through the governor.

### Phase B2: model pricing and cost budgets

- Add versioned pricing
- Implement conservative reservations and exact settlement
- Add parent total-workflow budget
- Add pricing validation and reconciliation tests

Exit criterion: a cost-capped run cannot make another call after its settled/reserved budget is spent.

### Phase B3: policy administration and audit foundation

- Add versioned policy repositories and APIs
- Snapshot effective policies on runs
- Add append-only governance events
- Add basic UI and operator diagnostics

Exit criterion: policy changes are attributable and historical runs remain reproducible.

### Phase B4: lifecycle policy engine

- Add metric windows and deterministic rules
- Add model/limit actions
- Add pause, cooldown, and interruption states
- Add transactional evaluation after run completion

Exit criterion: lifecycle actions affect subsequent dispatch and produce immutable decision records.

### Phase B5: definition versioning and rollback

- Add immutable agent versions
- Snapshot versions on runs
- Add policy-triggered rollback
- Define metric epochs

Exit criterion: rollback changes subsequent runs to a known immutable definition.

### Phase B6: durable approvals and input gates

- Generalize approval storage
- Segment executor into resumable operations
- Add approval/input branches and timeouts
- Add actor/reason audit events

Exit criterion: no gated side effect runs before approval, including across restarts.

### Phase B7: generalized workflow runtime

- Add workflow definitions, versions, instances, runs, and operation queue
- Add queue/coalesce modes and delayed operations
- Add code-handler compatibility versions
- Add interruption resolution

Exit criterion: a multi-operation workflow can pause, resume, retry, and roll back without process-local state.

### Phase B8: observability, UX, and hardening

- Add OTel hierarchy and redaction
- Add lifecycle and audit views if approved
- Load-test queue and policy queries
- Complete backup/restore and disaster-recovery tests

Exit criterion: production operational review passes.

### Phase B9: optional interactive-chat governance

- Add per-turn governance snapshots
- Preserve Redis SSE and stop/resume behavior
- Reuse native approval/input gates
- Measure first-token impact

Exit criterion: governed chat maintains accepted latency and streaming correctness.

## Testing strategy

### Unit tests

- Policy parsing and validation
- Effective-policy snapshots
- Call reservations and settlement
- Concurrent reservations
- Token/cache cost calculation
- Tool calls, denials, and failures
- Rule operators, windows, precedence, and conflicts
- State-machine transitions
- Audit payload validation

### Integration tests

Use existing testcontainers infrastructure with real Postgres/Redis:

- Runtime caps across every provider adapter
- Cost policy with versioned pricing
- Durable metrics under retry
- Scheduler respects pause/cooldown
- Lifecycle-selected model uses correct credentials
- Approval/input survives restart
- Rollback selects immutable definition version
- Multi-worker lease fencing prevents stale completion
- OTel spans correlate to run and audit IDs

### Failure-injection tests

Kill workers at:

- Before and after model reservation persistence
- During provider streaming
- After assistant tool request persistence
- After external side effect but before result persistence
- While awaiting approval/input
- During lifecycle evaluation
- During version-pointer update

### Property/state-machine tests

Generate operation transitions and assert:

- No terminal run returns to running
- Only the active claim token can mutate a leased operation
- A denied tool never produces an execution record
- Approval branches execute at most once per decision
- Audit events describe every lifecycle state transition
- Policy actions never expand agent permissions

## Rollout

1. Metrics-only governor
2. Enforced token/call/tool limits for internal non-mutating agents
3. Cost budgets after pricing validation
4. Opt-in policy UI
5. Lifecycle model/limit actions
6. Pause/cooldown
7. Approval/input gates
8. Version rollback
9. General workflows only when a product use case requires them

Each feature should have an independent flag. Once enforcement is enabled for an agent, repository or policy-engine failure must fail closed.

## Security and privacy

- Treat policies as authorization-adjacent configuration
- Require administrator authorization for organization-agent policy mutations
- Never allow policy-selected models to escape the configured allowlist
- Keep audit records append-only through repository permissions
- Redact trace content by default
- Keep OAuth/provider credentials out of workflow context and audit payloads
- Validate approval actor identity in Omni rather than trusting client-supplied strings
- Use idempotency keys for write tools

## Main risks

| Risk | Mitigation |
|---|---|
| Scope expands into a generic platform | Deliver runtime governor first; require a use case for each workflow feature |
| Policy semantics become ambiguous | Approve an RFC and exhaustive tests before UI work |
| Pricing becomes stale | Versioned admin-configured rates and visible source/effective date |
| Audit tables grow indefinitely | Partitioning/retention design after product approval |
| Lifecycle rules surprise operators | Effective-policy preview, deterministic precedence, full decision records |
| Side effects duplicate on replay | Tool idempotency contracts and reconciliation |
| Workflow engine increases queue complexity | Build on existing lease model; staged state-machine tests |
| Code rollback is falsely promised | Separate definition rollback from deployed-handler rollback |
| Governance code couples every provider | One common provider-call boundary and shared permit API |

## Rough effort

Assuming one experienced engineer and no interactive-chat integration:

- Runtime governor: 2-3 weeks
- Pricing and cost budgets: 2-4 weeks
- Policy APIs, persistence, and audit foundation: 3-5 weeks
- Lifecycle engine: 3-5 weeks
- Agent-definition versioning and rollback: 2-4 weeks
- Durable approval/input gates: 3-5 weeks
- Generalized workflow runtime: 5-9 weeks
- UI, observability, and hardening: 3-6 weeks

Indicative total for broad BoundFlow-like parity: **23-41 engineer-weeks**. The highest-value subset—runtime call/token/tool/timeout limits and cost budgets—is approximately **4-7 engineer-weeks**.

## Success criteria

- Every governed provider and tool call passes through one native boundary
- Enforcement never depends only on fire-and-forget usage persistence
- Missing models or prices fail before billable execution
- Policy edits cannot change an already-running run's snapshot
- Queue recovery and approval resumption preserve claim fencing
- Lifecycle decisions are deterministic and auditable
- Agent-definition rollback selects immutable historical configuration
- Workflow operations survive worker and service restarts
- Trace content is private by default
- No additional database server or external control plane is needed

## Go/no-go gates

Proceed from runtime governance to full workflow parity only if:

- Concrete Omni use cases require multi-operation workflows
- Maintainers approve the community/commercial placement
- The team accepts ownership of long-term policy and audit semantics
- Pricing maintenance has a clear owner
- Durable side-effect idempotency requirements are defined
- Runtime-governor adoption demonstrates value beyond the existing iteration limit
