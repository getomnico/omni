# Observability

Omni exports OpenTelemetry traces, metrics, and logs from the five core
services: **omni-web** (Node/SvelteKit), **omni-ai** (Python/FastAPI),
**omni-searcher** (Rust), **omni-indexer** (Rust), and
**omni-connector-manager** (Rust).  Connectors may capture telemetry
individually but are not audited for signal completeness.  This document
describes the signal architecture, metric inventory, and how to configure
an external collector or backend.

## Architecture & Signal Flow

```
┌──────────────────────────────────────────────────────────┐
│  Omni Core Services                                      │
│                                                           │
│  ┌──────────┐  ┌──────────┐  ┌──────┐  ┌──────┐  ┌────┐ │
│  │ Searcher  │  │ Indexer  │  │  AI  │  │ Web  │  │ CM  │ │
│  │ (Rust)    │  │ (Rust)   │  │(Python)│  │(Node)│  │Rust│ │
│  └────┬─────┘  └────┬─────┘  └──┬───┘  └──┬───┘  └──┬──┘ │
│       │              │           │         │         │     │
│       └──────────────┴───────────┴─────────┴─────────┘     │
│                                │  OTLP/HTTP                │
└────────────────────────────────┼───────────────────────────┘
                                 │
                    ┌────────────▼────────────────────┐
                    │  External OTLP/HTTP Collector   │
                    │  or Backend (user-managed)      │
                    └─────────────────────────────────┘
```

Omni does **not** bundle or require an OpenTelemetry Collector or backend.
Signals are sent to whatever endpoint is configured in
`OTEL_EXPORTER_OTLP_ENDPOINT`.  The default value is empty, which means
instrumentation is active but signals are **dropped / not exported or stored**.
When a non-empty endpoint is set, all three signals (traces, metrics, logs)
are sent to `${OTEL_EXPORTER_OTLP_ENDPOINT}/v1/{traces,metrics,logs}`.

Point `OTEL_EXPORTER_OTLP_ENDPOINT` at any compatible OTLP/HTTP collector
or backend. See [Production & Privacy Guidance](#production--privacy-guidance)
for production recommendations.

### Base OTLP Endpoint Semantics

| Environment Variable              | Default            | Description                                    |
|-----------------------------------|--------------------|------------------------------------------------|
| `OTEL_EXPORTER_OTLP_ENDPOINT`     | *(empty)*          | Base URL for OTLP HTTP export (e.g. `http://my-collector:4318`).  Empty = signals are dropped/instrumented but not exported. |
| `OTEL_DEPLOYMENT_ID`              | *(empty)*          | **Recommended:** set one shared non-empty value across all services. With Compose's empty value, Rust and Python export an empty `deployment.id`, while Node uses `unknown`. If the variable is absent entirely, Rust and Python generate a ULID. Set it explicitly for consistent cross-service identity. |
| `OTEL_DEPLOYMENT_ENVIRONMENT`     | `development`      | Environment label (`production`, `staging`, etc.) |
| `SERVICE_VERSION`                 | `0.1.0`            | Version label attached to all signals          |
| `OTEL_METRIC_EXPORT_INTERVAL`     | `60000`            | Metric export interval in milliseconds         |

### Resource Attributes

Every signal carries:
- `service.name` — the logical service name (`omni-searcher`, `omni-indexer`,
  `omni-ai`, `omni-web`, `omni-connector-manager`)
- `service.version` — from `SERVICE_VERSION`
- `deployment.environment` — from `OTEL_DEPLOYMENT_ENVIRONMENT`
- `deployment.id` — from `OTEL_DEPLOYMENT_ID`

### Shutdown Behaviour

On graceful shutdown (SIGTERM/SIGINT), each of the five core services flushes
pending spans, metrics, and logs before exiting.  In Rust services the OTLP
exporter timeout is hard-coded at 10 seconds in the opentelemetry-rust SDK;
`OTEL_EXPORTER_OTLP_TIMEOUT` is **not** honoured.  Busy shutdowns that exceed
the Rust SDK timeout may lose the final batch.  Python (omni-ai) honours the
standard OTLP timeout.  Node (omni-web) uses the SDK default.

## Trace Continuity

### HTTP Request Tracing

**Rust services** (searcher, indexer, connector-manager) use custom axum
middleware. SERVER spans use bounded `METHOD /route-template` names; custom
outbound CLIENT spans use `HTTP METHOD`. W3C `traceparent` / `tracestate` is
propagated in HTTP headers.

**Node (omni-web)** and **Python (omni-ai)** use auto-instrumentation provided
by the respective OpenTelemetry SDKs.  Span names follow the SDK convention
(e.g. HTTP method + route pattern) rather than a unified scheme.

Outbound HTTP requests create a CLIENT span, producing **parent/child
relationships within a single trace** (not links) when the web service calls
internal APIs.  The same `traceId` is shared across the web→searcher and
web→AI call chain.

### Async Queue Tracing

The indexer and connector-manager use a Postgres-based queue.

- **Connector-manager** enqueues connector events into `connector_events_queue`
  with a PRODUCER span, storing `traceparent` and `tracestate` in the queue row.
- **Indexer** consumes these events.  Because the producer and consumer run in
  different processes and transactions, the consumer creates a **new root trace**
  (no parent).  A **native OTel link** is added from the CONSUMER span back to
  the PRODUCER span via the stored `traceparent`/`tracestate` from the queue row.
- **Omni-ai batch processor** consumes embedding events from `embedding_queue`
  using the same producer→consumer pattern with `traceparent`/`tracestate` stored
  in queue rows and native OTel links.

This allows trace explorers to show the producer→consumer relationship even
though they are not in the same trace tree.

Queue instrumentation is implemented by the shared Rust queue/telemetry
helpers and service consumers, and by
`services/ai/embeddings/batch_processor.py` in Python.

## Metric Inventory

All metric names use the `omni.*` namespace.

### HTTP Server RED Metrics (cross-cutting, Rust services)

Recorded via axum middleware in `shared/src/metrics.rs`:

| Metric                                    | Type      | Description                              |
|-------------------------------------------|-----------|------------------------------------------|
| `omni.http.server.request_count`          | Counter   | Total HTTP requests by method, route, status |
| `omni.http.server.request_duration_seconds` | Histogram | Request duration in seconds              |

### Indexer Queue Metrics

Recorded in `shared/src/metrics.rs`:

| Metric                                    | Type      | Description                              |
|-------------------------------------------|-----------|------------------------------------------|
| `omni.indexer.events.processed`           | Counter   | Events successfully processed            |
| `omni.indexer.events.dead_letter`         | Counter   | Events moved to dead letter              |
| `omni.indexer.events.retried`             | Counter   | Events retried                           |
| `omni.indexer.batch.duration_seconds`     | Histogram | Time to process a batch                  |
| `omni.indexer.batch.size`                 | Histogram | Number of events in a processed batch    |
| `omni.indexer.queue.depth`                | Gauge     | Queue depth by status/queue attrs        |

### Searcher Metrics

Recorded in `shared/src/metrics.rs`:

| Metric                                    | Type      | Description                              |
|-------------------------------------------|-----------|------------------------------------------|
| `omni.searcher.search.duration_seconds`   | Histogram | Search request duration                  |
| `omni.searcher.search.results`            | Histogram | Number of results returned               |
| `omni.searcher.search.errors`             | Counter   | Search requests that errored             |
| `omni.searcher.cache.hit`                 | Counter   | Search requests served from cache        |
| `omni.searcher.cache.miss`                | Counter   | Search requests that missed cache        |

### Connector Sync Metrics

Recorded in `shared/src/metrics.rs`:

| Metric                                    | Type      | Description                              |
|-------------------------------------------|-----------|------------------------------------------|
| `omni.connector.sync.started`             | Counter   | Sync runs started                        |
| `omni.connector.sync.completed`           | Counter   | Sync runs completed                      |
| `omni.connector.sync.failed`              | Counter   | Sync runs failed                         |
| `omni.connector.sync.cancelled`           | Counter   | Sync runs cancelled                      |
| `omni.connector.sync.duration_seconds`    | Histogram | Sync run duration                        |

### AI / Embedding Metrics

Recorded in `services/ai/embeddings/batch_processor.py`:

| Metric                                    | Type      | Description                              |
|-------------------------------------------|-----------|------------------------------------------|
| `omni.ai.embedding.pending`               | Gauge     | Documents pending embedding              |
| `omni.ai.embedding.processed`             | Counter   | Documents successfully embedded          |
| `omni.ai.embedding.failed`                | Counter   | Documents that failed embedding          |
| `omni.ai.embedding.batch.duration`        | Histogram | Embedding batch processing duration      |

No LLM metrics are currently recorded.

Metrics for Rust services are defined centrally in `shared/src/metrics.rs`,
which all Rust services link.  The AI service instruments embedding metrics
locally in `batch_processor.py`.

## Log Correlation

How `trace_id` and `span_id` appear in log records depends on the runtime
and whether an active OTel span exists at the log call site.

| Runtime | Active span present | No active span                              |
|---------|---------------------|---------------------------------------------|
| Rust    | `trace_id`/`span_id` attached via `tracing-subscriber` with `with_current_span(true)` | Correlation fields omitted entirely |
| Python  | `trace_id`/`span_id` set from active span | Zeros (`000...0`) for both fields           |
| Node    | `trace_id`/`span_id` attached via Pino `hooks.logMethod` | Correlation fields omitted entirely |

### Sanitisation

- Log body content is sent to the OTLP endpoint exactly as produced by the
  application.  **No global PII scrubber is applied.**
- Node's explicit Pino-to-OTel hook exports only allowlisted primitive log
  attributes. Its error serializer removes dynamic messages and stacks from
  structured error fields. Automatic Pino log sending is disabled to prevent
  duplicate export.
- Runtime logging call sites were audited to avoid sensitive and
  high-cardinality fields, but this is not a global scrubber. Future log sites
  must follow the same policy.

## Troubleshooting

| Symptom                          | Likely Cause & Fix                                        |
|----------------------------------|-----------------------------------------------------------|
| No traces/metrics/logs exported  | `OTEL_EXPORTER_OTLP_ENDPOINT` is empty or unreachable.  Set it to the URL of your OTLP/HTTP collector or backend. |
| Telemetry lost on shutdown       | OTLP exporter timeout too short for pending data.  Rust services use a hard-coded 10 s timeout (`OTEL_EXPORTER_OTLP_TIMEOUT` is not honoured).  Python honours the env var.  Ensure shutdown completes within the timeout. |
| Invalid `traceparent` header     | Client libraries generate invalid W3C TraceContext.  Check that the `traceparent` header is formatted as `00-{trace_id}-{span_id}-{flags}`. |
| High metric cardinality          | Dynamic label values (e.g. user IDs, search queries) create many time series.  Configure your collector or backend with appropriate sampling or cardinality limits. |
| Log body contains sensitive data | No PII scrubbing is applied.  Filter sensitive fields at the application level or configure your collector/backend to redact them. |

## Production & Privacy Guidance

When using an external OTLP collector or backend in production:

1. **Use TLS** — enable TLS on the OTLP endpoint or use gRPC with TLS.
2. **Authenticate** — use API keys or mTLS to secure the connection.
3. **Configure sampling** — use a `probabilistic_sampler` or `tail_sampling`
   processor in your collector for high-throughput services.
4. **Adjust batching** — tune batch processor timeouts and sizes for your
   throughput.
5. **Set resource limits** — configure memory limits and spike limits in
   the collector or backend.
6. **Never expose the OTLP endpoint publicly** — it has no built-in
   authentication. Bind to loopback or use a private network.
7. **Filter sensitive data** — no PII scrubbing is applied by Omni.  Redact
   sensitive fields in the collector or at the application level before
   logging.
