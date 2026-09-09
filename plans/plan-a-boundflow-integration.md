# Plan A: Integrate BoundFlow as Omni's Agent Governance Control Plane

## Status

Proposed for comparison. This is not an implementation commitment.

Before implementation, maintainers must confirm which governance and audit features belong in the community edition. Basic self-hosted runtime safety limits are likely community functionality; organization-wide policy administration and audit reporting may fall under Omni's commercial-edition guidance.

## Executive summary

Use BoundFlow as the policy, workflow-lifecycle, governance-metrics, approval, and audit control plane for Omni agents. Omni continues to own agent definitions, provider credentials, tools, conversation logs, and user-facing run history. BoundFlow becomes the execution authority for governed agent runs and invokes an SDK worker hosted by `omni-ai`.

This approach buys a functioning control plane and policy engine, but introduces a second durable state machine and a young pre-1.0 dependency. The integration must make ownership boundaries explicit and must never silently bypass BoundFlow when governance is enabled.

## Goals

Deliver the BoundFlow capabilities that are relevant to Omni:

- Runtime policies:
  - Maximum LLM calls per run
  - Maximum cost per run
  - Maximum output tokens per call
  - Maximum model-call duration
  - Per-tool call limits
  - Per-tool failure limits
  - Policy-selected models
  - Application-defined custom limits
- Agent lifecycle policies based on recent-run metrics:
  - Model switching
  - LLM-call limit changes
  - Cost-limit changes
  - Token-limit changes
- Workflow lifecycle policies:
  - Pause
  - Cooldown
  - Workflow-handler version rollback
- Durable execution and operation resumption
- Durable approval and input gates
- Governance metrics and policy-decision audit records
- OpenTelemetry-compatible operation, LLM, and tool traces
- External policy administration through BoundFlow's API, CLI, and console

## Non-goals

- Sending Omni's model-provider credentials or raw inference traffic to the BoundFlow backend
- Replacing Omni's connector, search, sandbox, memory, or model-provider layers
- Moving Omni chat history or agent conversation WAL into BoundFlow
- Sharing one SQL schema between Omni and BoundFlow
- Silently running an ungoverned agent when BoundFlow is enabled but unavailable
- Applying BoundFlow to interactive chat in the first production release

## Product scope

### Initial scope

- Background and scheduled agents implemented under `services/ai/agents/`
- Manual background-agent triggers
- Agent tool execution
- Auxiliary agent calls for compaction and run summaries

### Later scope

- Interactive chat turns in `services/ai/streaming/generate.py`
- Durable approvals for action-bearing interactive workflows
- Multi-operation workflows authored by Omni or connector capabilities

Interactive chat should remain outside the first cutover because it has a separate streaming lifecycle and user-disconnect semantics. A later design can invoke a BoundFlow workflow per chat turn while continuing to stream through Omni's Redis stream.

## BoundFlow dependency baseline

The reviewed release is BoundFlow `0.7.0`.

- Python SDK: MIT
- Backend repository: Apache-2.0
- SDK requirements are compatible with Omni's Python 3.12 runtime and current protobuf 6.x lock
- BoundFlow describes itself as a public preview and states that its APIs may change before 1.0

Requirements:

- Pin the Python SDK exactly rather than using an open range
- Pin the backend image by version or digest rather than using `latest`
- Put all BoundFlow-specific code behind an Omni-owned interface
- Add a compatibility test before every BoundFlow version upgrade

## Target architecture

```text
Omni web / scheduler
        |
        | creates agent_runs
        v
Omni governance dispatcher
        |
        | invoke_workflow(context={agent_run_id})
        v
BoundFlow server --> BoundFlow Postgres database
        |
        v
BoundFlow scheduler --> BoundFlow worker router
        |
        | bidirectional gRPC
        v
BoundFlowWorker hosted by omni-ai
        |
        | OperationContext + AgentGovernor
        v
Omni agent executor --> Omni tools/providers/agent WAL
        |
        v
Omni Postgres database
```

### Authority boundaries

| Concern | Source of truth / writer |
|---|---|
| Agent definition and permissions | `omni-web` / Omni `agents` tables |
| Agent run presentation and conversation WAL | `omni-ai` / Omni `agent_runs` and `agent_run_logs` |
| Model credentials | Omni encrypted model-provider configuration |
| Runtime and lifecycle policy | BoundFlow |
| Workflow lifecycle and governance audit | BoundFlow |
| Provider token usage reports | Omni and BoundFlow, reconciled by request ID |
| Prompt and tool trace export | Omni-owned OTel sink; disabled or redacted by default |

BoundFlow does not receive inference credentials or make provider calls. The SDK worker runs inside `omni-ai` and continues to call Omni's provider implementations.

## Database topology

BoundFlow uses its own Postgres tables and migrations. It must not use Omni's current database because both projects define unqualified tables such as `api_keys`.

Recommended topology:

```text
One PostgreSQL/ParadeDB server
  - database: omni       # existing Omni migrations and data
  - database: boundflow  # BoundFlow-owned migrations and data
```

Use a dedicated database role for BoundFlow. Point all BoundFlow backend modes at:

```text
postgresql://boundflow:<secret>@postgres:5432/boundflow
```

Omni services continue using their existing `DATABASE_*` settings. The BoundFlow Python SDK communicates over gRPC and does not connect directly to the BoundFlow database.

Add an idempotent database-bootstrap job that creates the role and logical database before the BoundFlow migration job. Production deployments may instead supply a managed `BOUNDFLOW_DATABASE_URL`.

Using a separate BoundFlow Postgres container is supported but is not the default recommendation because it adds another database server. PostgreSQL 17/ParadeDB compatibility must be proven in the integration suite because BoundFlow's sample deployment currently uses PostgreSQL 16.

## Omni data-model changes

### AI-owned binding table

Add `agent_governance_bindings`, written only by `omni-ai`:

- `agent_id`
- `backend` (`boundflow`)
- `external_tenant_id`
- `external_workflow_id`
- `workflow_type`
- `workflow_version`
- `provisioning_status`
- `last_error`
- `created_at`
- `updated_at`

There is one BoundFlow workflow instance per Omni agent. This gives each agent independent policy history, metrics, pause/cooldown state, and audit records.

### Agent-run correlation

Extend `agent_runs` with:

- `governance_backend`
- `governance_request_id`
- `governance_status`
- `governance_failure_reason`

The BoundFlow request ID is the cross-system idempotency and trace-correlation key.

### Version metadata

Record the Omni executor compatibility version used for each run. BoundFlow's `SetVersion` changes the workflow-handler version, not an arbitrary mutable Omni agent definition. Supporting rollback requires old handler versions to remain registered and deployable.

If user-editable agent-definition rollback is required, add immutable `agent_versions` separately; do not pretend BoundFlow handler rollback automatically versions Omni prompts, permissions, or schedules.

## Integration modules

Add an Omni-owned package:

```text
services/ai/governance/
  __init__.py
  base.py
  models.py
  boundflow_client.py
  boundflow_dispatcher.py
  boundflow_worker.py
  boundflow_session.py
  model_resolution.py
```

### Internal interface

`base.py` should define concrete protocols independent of BoundFlow:

- `GovernanceBackend`
- `GovernedRunContext`
- `ModelCallPermit`
- `ToolCallPermit`
- `GovernanceOutcome`

The existing agent executor should depend on these interfaces rather than importing BoundFlow throughout the loop. This preserves the option to disable or replace BoundFlow later.

## Execution and dispatch design

### Normal flow

1. Omni's schedule materializer or manual trigger creates an `agent_runs` row.
2. The governance dispatcher finds undispatched pending rows.
3. It ensures the agent has an active BoundFlow workflow binding.
4. It invokes that workflow with the Omni run ID in `initial_context`.
5. It stores the returned BoundFlow request ID.
6. BoundFlow dispatches the generic Omni handler to a connected `BoundFlowWorker` in `omni-ai`.
7. The handler claims the exact Omni run using the BoundFlow request ID.
8. The handler runs the existing Omni executor under one or more `AgentGovernor` instances.
9. Omni durably records assistant tool requests before executing tools and records results immediately afterward.
10. The handler completes or fails the Omni run, then returns a BoundFlow `Complete` result.
11. BoundFlow stores metrics, outcome, lifecycle decisions, audit records, and traces.

### Existing queue behavior

When `BOUNDFLOW_ENABLED=true`, `run_agent_queue_worker` must not execute pending runs directly. A governed and ungoverned consumer must never race for the same row.

The schedule materializer may remain because Omni supports its existing cron and interval semantics. BoundFlow becomes the execution dispatcher and lifecycle authority, not necessarily the schedule author.

### Idempotency and resumption

Add an exact-run claim operation fenced by:

- Omni run ID
- BoundFlow request ID
- Fresh Omni claim token

Handler behavior:

- Completed Omni run: return its existing outcome without rerunning
- Running under the same live claim: reject duplicate concurrent dispatch
- Stale running claim for the same BoundFlow request: acquire a new claim and resume from the WAL
- Different request attached to the same active run: fail closed and alert

Enable BoundFlow's resumable workflow setting only after crash/replay tests prove this contract. Omni's existing WAL makes model/tool progression recoverable, but external side effects still need idempotency keys or reconciliation behavior.

## Runtime-governance integration

Refactor `services/ai/agents/executor.py` so every provider call obtains a permit.

### Model call sequence

For every model attempt:

1. `call = governor.begin_call()`
2. `await ctx.report_metrics()` to report the pre-call reservation
3. Resolve `call.model` to an active Omni model and provider
4. Use no more than `call.max_tokens`
5. Wrap full stream consumption in `asyncio.timeout(call.timeout_seconds)` when nonzero
6. Capture provider-reported input, output, cache-read, and cache-creation tokens
7. On provider failure before a billable response, call `call.abandon()`
8. On success, call `call.record(Usage(...), tool_calls=[...])`
9. `await ctx.report_metrics()` to replace the reservation with actual usage
10. Persist the same actual usage to Omni's `model_usage`

Context-overflow retries count as separate model attempts only if the provider reports billable usage. Define and test this boundary for every provider adapter.

### Governor names

Use stable names within each per-agent workflow:

- `responder`: the main reasoning/tool loop
- `compactor`: conversation compaction calls
- `run_summary`: final run summary calls

Separate names prevent hidden auxiliary calls from being attributed to the wrong model and pricing. Operators must understand that a responder-only cap is not a total-workflow runtime cap. Workflow metrics aggregate all three after completion.

### Tool call sequence

Wrap `registry.execute(...)`:

1. Call `governor.begin_tool_call(tool_name, call_id=...)`.
2. If denied, do not execute the tool; return the denial message as an error tool result.
3. Execute allowed tools through Omni's registry.
4. Record output or failure on the governed tool call.
5. Persist the result to the Omni WAL as today.

Invalid model-generated JSON does not count as an executed tool. OAuth and user-denied actions also do not count as successful executions.

BoundFlow cannot enforce limits on tools not passed through this boundary.

### Custom policy fields

BoundFlow stores but does not enforce `RuntimePolicy.custom`. Add explicit Omni handlers for any supported custom fields, for example:

- Maximum total fetched bytes
- Maximum sandbox runtime
- Maximum connector writes
- Maximum number of sources accessed

Reject unknown custom fields if the UI claims they are enforced.

## Model selection and pricing

### Model resolution

`call.model` is a policy identifier, while Omni must select a provider client and credentials.

Initial strategy:

- Store configured provider wire-model names in BoundFlow policies.
- Add exact lookup by `models.model_id` in `ProviderCache`.
- Require exactly one active Omni model record for a policy-selected wire name.
- Fail closed if the model is missing or ambiguous.

A later alias format such as `omni-model:<ULID>` can remove ambiguity, but aliases require matching prices to be configured in BoundFlow.

### Pricing

Cost caps are valid only when BoundFlow has pricing for every allowed model.

Provisioning checks must:

- Query BoundFlow's effective pricing table
- Verify input and output prices exist
- Verify cache pricing semantics where relevant
- Reject a cost policy for an unpriced model
- Surface custom/self-hosted models as requiring explicit pricing

Omni and BoundFlow cost totals should be reconciled in integration tests. BoundFlow's total is governance-authoritative; Omni's remains the user-facing usage ledger.

### Token defaults

BoundFlow defaults uncapped calls to 4096 output tokens, while Omni currently defaults to 8192. Provision each workflow with an explicit runtime default matching Omni, or accept and document the changed default. Summary calls may request less than the policy maximum.

## Durable approvals and input gates

### Tool approval flow

1. The model emits a tool request.
2. Omni durably records the assistant message and tool input.
3. The BoundFlow operation returns `AwaitApproval` before the tool runs.
4. Omni displays the pending approval using the BoundFlow approval ID.
5. Approve/reject requests go through an authenticated Omni API that calls BoundFlow.
6. BoundFlow dispatches the selected branch.
7. The resumed operation reloads the Omni WAL and either executes the tool or records a denial result.

Approval actors and reasons are written to BoundFlow's audit log. Omni stores the minimum correlation metadata needed for its UI.

### OAuth

OAuth is not a binary governance decision and remains owned by Omni. If represented as a BoundFlow input gate, only opaque OAuth state references may be placed in BoundFlow context; access tokens and credentials must stay in Omni's encrypted credential store.

### Operation boundaries

A gate requires ending one BoundFlow operation and resuming another. The agent loop must therefore be refactored into resumable segments rather than holding an in-memory stack across approval.

## Lifecycle policies

### Agent lifecycle

Expose BoundFlow rules for:

- Recent cost
- Recent token use
- LLM calls
- Calls per tool

Supported actions should modify the effective runtime policy for the next run. Policy-selected models must pass Omni model-resolution and pricing validation.

### Workflow lifecycle

Map BoundFlow state into Omni agent presentation:

- BoundFlow paused -> Omni agent cannot dispatch new runs
- BoundFlow cooldown -> show resume time and prevent dispatch
- BoundFlow interrupted -> show operator action required
- BoundFlow active -> normal scheduling

Do not duplicate these decisions by setting `agents.is_enabled`; that field is user intent, while BoundFlow state is runtime governance state.

### Version rollback

Register each supported execution-handler version with the SDK worker. Keep old handler implementations available during the rollback window. A rollback changes execution behavior only if the target version is actually deployed and registered.

Agent-definition snapshots require separate Omni versioning and explicit mapping to handler versions.

## Observability and audit

### Metrics

Expose correlated views containing:

- Omni agent run status and summary
- BoundFlow workflow/request IDs
- Per-governor cost, tokens, calls, and tool counts
- Effective model and runtime policy
- Lifecycle actions applied

### Traces

Use BoundFlow's OTel trace sink with Omni's existing telemetry initialization. Prompt and response content must be disabled, redacted, or explicitly opted into because it may contain workplace data.

### Audit

Initially use BoundFlow's CLI/console for governance audit inspection. An Omni audit-reporting UI or export is a separate product decision requiring maintainer confirmation.

## API and UI work

### Initial administration

- Configure policies using BoundFlow CLI or console
- Show BoundFlow connection and workflow IDs in an operator diagnostics endpoint
- Show governed run status and budget-termination reason in Omni

### Native Omni administration

Later add authenticated Omni endpoints that proxy typed operations through `ControlPlaneClient`:

- Get/set runtime policy
- Get/set lifecycle policy
- Pause/resume/resolve interrupted workflow
- Approve/reject/submit input
- Read run metrics and policy-decision records

Do not let `omni-web` access BoundFlow's database directly.

## Deployment plan

Add optional services to Docker Compose:

- `boundflow-db-init`
- `boundflow-migrate`
- `boundflow-server`
- `boundflow-scheduler`
- `boundflow-worker-router`

Add configuration:

- `BOUNDFLOW_ENABLED`
- `BOUNDFLOW_DATABASE_URL`
- `BOUNDFLOW_SERVER_ADDRESS`
- `BOUNDFLOW_WORKER_ADDRESS`
- `BOUNDFLOW_API_KEY`
- `BOUNDFLOW_IMAGE`
- `BOUNDFLOW_WORKFLOW_VERSION`

Keep gRPC ports internal to `omni-network`. If BoundFlow is remote, require TLS termination and validate server certificates.

Track the BoundFlow SDK worker task during application startup and cancel it cleanly during shutdown. Multiple `AI_WORKERS` processes may register the same capabilities only after multi-worker dispatch behavior is verified.

## Failure semantics

| Failure | Required behavior |
|---|---|
| BoundFlow disabled | Existing Omni execution path remains available |
| BoundFlow enabled but unreachable | Governed run remains pending/blocked and does not execute |
| SDK worker disconnected | BoundFlow reports blocked/interrupted state; Omni does not bypass it |
| Provider fails before response | Abandon call reservation; use Omni retry policy |
| Provider times out | Cancel the call and record a failed governed run |
| Policy-selected model missing | Fail run explicitly before provider call |
| Pricing missing with cost cap | Reject policy/provisioning or fail before execution |
| Omni worker crashes mid-tool | Resume from WAL; reconcile potentially duplicated side effect |
| BoundFlow callback repeats | Exact-run claim and request ID prevent concurrent execution |
| Metric persistence differs | Alert on reconciliation mismatch; do not rewrite historical audit records |

## Implementation phases

### Phase A0: compatibility spike

- Start pinned BoundFlow services locally
- Connect an SDK worker from `omni-ai`
- Run a mocked generic workflow
- Verify protobuf and gRPC dependency compatibility
- Verify BoundFlow migrations on the selected Postgres topology
- Demonstrate call, token, timeout, tool, and cost caps
- Produce a go/no-go report

Exit criterion: no changes to normal Omni execution and all cap semantics demonstrated end to end.

### Phase A1: governance abstraction and call instrumentation

- Add Omni-owned governance interfaces
- Add BoundFlow adapter
- Refactor model and tool boundaries in `executor.py`
- Preserve Omni usage tracking
- Add model and pricing validation

Exit criterion: existing executor tests pass with a no-op backend and BoundFlow unit tests pass with fakes.

### Phase A2: dispatch, bindings, and durable correlation

- Add migrations
- Provision one workflow per agent
- Add dispatcher and exact-run claim
- Disable direct queue consumption in BoundFlow mode
- Add reconciliation and operator diagnostics

Exit criterion: manual and scheduled runs complete through BoundFlow without duplicate execution.

### Phase A3: approvals and resumable operation segments

- Split execution around action gates
- Wire approval/input APIs
- Resume from Omni WAL
- Correlate actors and reasons

Exit criterion: no governed write tool executes before approval, including after restart.

### Phase A4: lifecycle policies and versioning

- Surface pause/cooldown/interrupted states
- Validate model-switch actions
- Register multiple handler versions
- Exercise version rollback

Exit criterion: lifecycle decisions change subsequent runs and produce durable audit entries.

### Phase A5: observability and administration

- Integrate OTel sink with content redaction
- Add metrics/status UI
- Add typed policy management APIs if approved
- Document backup, restore, upgrade, and disaster recovery

Exit criterion: operators can diagnose a run across both systems using one correlation ID.

### Phase A6: optional interactive-chat support

- Design one workflow invocation per chat turn
- Preserve Redis SSE behavior
- Define disconnect, stop, resume, and approval semantics
- Load-test latency and workflow cardinality

Exit criterion: governed chat does not regress first-token latency beyond the accepted budget and never loses streamed output.

## Testing strategy

### Unit tests

- Call permit lifecycle: begin, record, abandon, duplicate record
- Timeout behavior
- Cache-token mapping
- Model lookup and ambiguity failures
- Tool denial and failure counting
- BoundFlow-to-Omni outcome mapping
- Policy validation

### Integration tests

Use real Postgres/Redis plus pinned BoundFlow containers:

- Runtime policy fetched from BoundFlow stops an Omni loop
- Tool cap prevents registry execution
- Model lifecycle policy switches to the configured provider
- Cost totals match provider usage
- Approval survives process restart
- Stale Omni claim resumes under the same BoundFlow request
- Repeated callback does not duplicate completion
- Pause and cooldown stop scheduling
- Version rollback dispatches a registered old handler

### Failure-injection tests

Kill each component at these boundaries:

- Before external invoke is recorded
- After BoundFlow schedules but before SDK dispatch
- After model call begins
- After assistant tool request is persisted
- After tool side effect but before result persistence
- After Omni completion but before BoundFlow completion

## Rollout

1. Development-only feature flag
2. Shadow metrics with policies configured but unenforced
3. Internal agents with non-mutating tools
4. Internal agents with idempotent write tools
5. Opt-in self-hosted preview
6. Default-on only after BoundFlow stability and recovery criteria are met

Once an agent opts into enforcement, unavailability must fail closed. Shadow mode must be visually distinct from enforced mode.

## Security and privacy

- Store BoundFlow API keys in service secrets, never Omni's normal database in plaintext
- Keep provider keys entirely inside Omni
- Pass only opaque IDs and small coordination data in BoundFlow workflow context
- Redact prompt, response, tool input, and tool output trace content by default
- Authenticate and authorize all approval and policy mutations through Omni
- Use dedicated BoundFlow database credentials
- Back up and restore the BoundFlow database consistently with Omni correlation records

## Main risks

| Risk | Mitigation |
|---|---|
| Pre-1.0 API churn | Exact pins, adapter boundary, compatibility suite |
| Two durable state machines diverge | Explicit authority matrix, request IDs, reconciliation worker |
| Added operational burden | Same Postgres server with separate DB; optional profile; health diagnostics |
| Cost caps ineffective for custom models | Mandatory pricing validation |
| Duplicate external side effects on replay | Idempotency keys and tool-specific reconciliation |
| Version rollback misunderstood | Document handler-version semantics and retain old workers |
| Sensitive trace content exported | Redaction/default-off content capture |
| BoundFlow outage blocks agents | Intentional fail-closed behavior, clear operator status |
| Dependency becomes unavailable | Omni abstraction and documented migration/off-ramp |

## Rough effort

Assuming one experienced engineer, existing infrastructure familiarity, and no interactive-chat integration:

- Compatibility spike: 1-2 weeks
- Runtime adapter and model/tool governance: 2-3 weeks
- Durable dispatch and reconciliation: 3-5 weeks
- Approval/input gates: 2-4 weeks
- Lifecycle/version support: 2-4 weeks
- UI, operations, and hardening: 2-4 weeks

Indicative total: **12-22 engineer-weeks**, plus ongoing dependency and deployment maintenance. Interactive chat would be an additional project.

## Success criteria

- Every governed provider and tool call passes through BoundFlow enforcement
- No enabled policy can be bypassed because BoundFlow is unavailable
- Omni and BoundFlow agree on calls and token usage for test runs
- Cost policies cannot run against unknown pricing
- Crashes and repeated dispatch do not produce duplicate run completion
- Approval gates survive restart and prevent premature side effects
- Pause, cooldown, model switching, and version rollback work end to end
- Operators can correlate Omni runs, BoundFlow requests, metrics, audit records, and traces
- BoundFlow can be disabled without corrupting existing Omni run history

## Go/no-go gates

Proceed beyond the spike only if:

- BoundFlow accepts the selected Postgres topology
- Its pre-1.0 upgrade policy is acceptable
- The team accepts fail-closed dependence on its control plane
- The cross-system recovery tests pass
- Pricing coverage is adequate for Omni's supported models
- Maintainers approve the community/commercial placement of policy and audit features
