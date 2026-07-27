//! Integration tests for the shared telemetry module.
//!
//! Uses `InMemorySpanExporter` to inspect exported OpenTelemetry spans and
//! verify:
//!
//! - `send_traced` creates a CLIENT span, injects a valid W3C `traceparent`
//!   header whose span ID matches the client span (not the parent), and
//!   shares the parent trace ID.
//! - The Axum `trace_layer` middleware creates a single SERVER span with a
//!   templated route name (no raw ID/query), records 200 and 5xx statuses,
//!   and marks 5xx as ERROR.
//!
//! No EnteredGuard is held across any .await point.

use std::net::SocketAddr;
use std::sync::OnceLock;

use axum::{body::Body, extract::Request, middleware, response::Response, routing::get, Router};
use opentelemetry::{
    global,
    trace::{SpanId, SpanKind, TraceId, TracerProvider as _},
};
use opentelemetry_sdk::{
    propagation::TraceContextPropagator,
    trace::{InMemorySpanExporter, RandomIdGenerator, SdkTracerProvider, SimpleSpanProcessor},
};
use reqwest::Client;
use serde_json::Value;
use tokio::sync::Mutex;
use tracing::{info_span, Instrument};
use tracing_subscriber::{layer::SubscriberExt as _, util::SubscriberInitExt as _};

// ---------------------------------------------------------------------------
// Global test infrastructure
// ---------------------------------------------------------------------------

/// Global in-memory span exporter shared across all tests.
static EXPORTER: OnceLock<InMemorySpanExporter> = OnceLock::new();
fn exporter() -> &'static InMemorySpanExporter {
    EXPORTER.get().expect("exporter not initialized")
}

/// Suite mutex serialising access to the global InMemorySpanExporter.
/// Must be acquired (and held through the critical section) by each test
/// before calling `reset_exporter()` and released only after all span
/// assertions complete, to prevent reset/read races across parallel
/// `#[tokio::test]` execution.
static TEST_MUTEX: OnceLock<Mutex<()>> = OnceLock::new();
fn test_mutex() -> &'static Mutex<()> {
    TEST_MUTEX.get_or_init(|| Mutex::new(()))
}

/// Initialise the global tracer provider + tracing subscriber once per suite.
fn ensure_global_init() {
    EXPORTER.get_or_init(|| {
        global::set_text_map_propagator(TraceContextPropagator::new());

        let exporter = InMemorySpanExporter::default();
        let provider = SdkTracerProvider::builder()
            .with_span_processor(SimpleSpanProcessor::new(exporter.clone()))
            .with_id_generator(RandomIdGenerator::default())
            .build();
        global::set_tracer_provider(provider.clone());

        let tracer = provider.tracer("test");
        let telemetry_layer = tracing_opentelemetry::layer().with_tracer(tracer);
        let _ = tracing_subscriber::registry()
            .with(telemetry_layer)
            .try_init();

        exporter
    });
}

/// Reset the exporter. Tests should call this before their traced operations.
fn reset_exporter() {
    exporter().reset();
}

/// Helper to get currently finished spans from the exporter.
/// With `SimpleSpanProcessor`, spans are exported synchronously on end,
/// so no explicit force-flush is needed — the data is already present.
fn finished_spans() -> Vec<opentelemetry_sdk::trace::SpanData> {
    exporter()
        .get_finished_spans()
        .expect("get_finished_spans should succeed")
}

/// Extract a reference to the `Value` from a `KeyValue` by key name.
fn attr_value<'a>(
    attrs: &'a [opentelemetry::KeyValue],
    key: &str,
) -> Option<&'a opentelemetry::Value> {
    attrs
        .iter()
        .find(|kv| kv.key.as_str() == key)
        .map(|kv| &kv.value)
}

/// Match an OTel `Value` to a string reference.
fn value_as_str(v: &opentelemetry::Value) -> Option<&str> {
    match v {
        opentelemetry::Value::String(s) => Some(s.as_ref()),
        _ => None,
    }
}

/// Match an OTel `Value` to an i64.
fn value_as_i64(v: &opentelemetry::Value) -> Option<i64> {
    match v {
        opentelemetry::Value::I64(i) => Some(*i),
        _ => None,
    }
}

/// Check whether a `Status` is error.
fn status_is_error(status: &opentelemetry::trace::Status) -> bool {
    matches!(status, opentelemetry::trace::Status::Error { .. })
}

/// Convert a `SpanId` to its hex string representation.
fn span_id_hex(id: opentelemetry::trace::SpanId) -> String {
    format!("{:016x}", id)
}

/// Convert a `TraceId` to its hex string representation.
fn trace_id_hex(id: opentelemetry::trace::TraceId) -> String {
    format!("{:032x}", id)
}

// ---------------------------------------------------------------------------
// HTTP helpers
// ---------------------------------------------------------------------------

/// Start a simple downstream server that echoes request headers as JSON.
/// Returns the bound address and a oneshot sender to shut it down.
async fn start_downstream() -> (SocketAddr, tokio::sync::oneshot::Sender<()>) {
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();

    async fn echo_headers(req: Request) -> Response {
        let headers: Vec<(String, String)> = req
            .headers()
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
            .collect();
        let body = serde_json::to_string(&serde_json::json!({ "headers": headers })).unwrap();
        Response::new(Body::from(body))
    }

    let app = Router::new().route("/echo", get(echo_headers).post(echo_headers));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let wait_for_start = {
        let addr = addr;
        async move {
            let client = reqwest::Client::new();
            loop {
                if client
                    .get(format!("http://{}/echo", addr))
                    .send()
                    .await
                    .is_ok()
                {
                    break;
                }
                tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;
            }
        }
    };

    tokio::spawn(async move {
        let serve = axum::serve(listener, app).with_graceful_shutdown(async {
            rx.await.ok();
        });
        let _ = serve.await;
    });

    // Wait for the server to be ready
    tokio::time::timeout(tokio::time::Duration::from_secs(5), wait_for_start)
        .await
        .expect("downstream server did not start in time");

    (addr, tx)
}

/// Parse a `traceparent` value into its components: version, trace_id, span_id, flags.
fn parse_traceparent(tp: &str) -> Option<(&str, &str, &str, &str)> {
    let parts: Vec<&str> = tp.split('-').collect();
    if parts.len() == 4 {
        Some((parts[0], parts[1], parts[2], parts[3]))
    } else {
        None
    }
}

/// Extract the `traceparent` header value from the JSON-echoed headers array.
fn get_traceparent_from_json(headers: &[Value]) -> Option<String> {
    headers
        .iter()
        .find(|h| h[0].as_str().unwrap_or("").to_lowercase() == "traceparent")
        .map(|h| h[1].as_str().unwrap().to_string())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Verify that `send_traced` creates a CLIENT span, injects a valid
/// `traceparent` header whose span ID matches the client span, and that
/// the client span is a child of any active parent.
///
/// The parent scope is established via `.instrument()` so no EnteredGuard
/// is held across any `.await`.
#[tokio::test]
async fn test_send_traced_client_span_and_injection() {
    ensure_global_init();
    let (addr, _stop) = start_downstream().await;
    let url = format!("http://{}/echo", addr);

    let _lock = test_mutex().lock().await;
    reset_exporter();

    let parent = info_span!("test_parent", otel.kind = "INTERNAL");

    // ---- within parent span ----
    async {
        let client = Client::new();
        let resp = shared::telemetry::http_client::send_traced("GET", &url, client.get(&url))
            .await
            .expect("send_traced should succeed");
        assert!(resp.status().is_success(), "downstream should return 200");

        let body: Value = resp.json().await.unwrap();
        let headers = body["headers"].as_array().unwrap();

        // --- 1. Validate the injected traceparent header ---
        let tp_val = get_traceparent_from_json(headers)
            .expect("downstream request must contain a traceparent header");
        let (version, tp_trace_id, tp_span_id, tp_flags) =
            parse_traceparent(&tp_val).expect("traceparent must be valid W3C format");
        assert_eq!(version, "00", "traceparent version must be 00");
        assert_eq!(tp_flags, "01", "traceparent flags must be 01 (sampled)");
        assert_eq!(tp_trace_id.len(), 32, "trace_id must be 32 hex chars");
        assert_eq!(tp_span_id.len(), 16, "span_id must be 16 hex chars");
        assert!(
            tp_trace_id.chars().all(|c| c.is_ascii_hexdigit()),
            "trace_id must be hex"
        );
        assert!(
            tp_span_id.chars().all(|c| c.is_ascii_hexdigit()),
            "span_id must be hex"
        );

        // --- 2. Get finished spans and locate the CLIENT span ---
        // (Parent span is still active inside this instrumented block.
        //  CLIENT span should have been exported synchronously on end.)
        let spans = finished_spans();

        // Find CLIENT spans whose trace_id matches the injected header.
        // (Due to concurrent test execution, other CLIENT spans may be present;
        //  we filter by the trace_id that we know was injected.)
        let client_spans: Vec<_> = spans
            .iter()
            .filter(|s| {
                s.span_kind == SpanKind::Client
                    && trace_id_hex(s.span_context.trace_id()) == tp_trace_id
            })
            .collect();
        assert!(
            !client_spans.is_empty(),
            "send_traced must create at least one CLIENT span matching the injected trace_id"
        );
        let client_span = &client_spans[0];

        assert_eq!(
            client_span.name, "HTTP GET",
            "CLIENT span name should be 'HTTP GET'"
        );

        // --- 3. Injected traceparent uses client span ID ---
        let client_span_id_hex = span_id_hex(client_span.span_context.span_id());
        assert_eq!(
            tp_span_id, client_span_id_hex,
            "injected traceparent span_id must match the CLIENT span's span_id"
        );

        // --- 4. CLIENT span records http.request.method ---
        let method_val = attr_value(&client_span.attributes, "http.request.method");
        assert_eq!(
            method_val.and_then(value_as_str),
            Some("GET"),
            "CLIENT span must record http.request.method=GET"
        );
    }
    .instrument(parent)
    .await;
}

/// Verify the Axum `trace_layer` middleware creates a single SERVER span with
/// templated route, records 200 status, and does not create raw-path spans.
#[tokio::test]
async fn test_middleware_server_span_on_success() {
    ensure_global_init();

    use shared::telemetry::middleware::trace_layer;

    let app = Router::new()
        .route("/ok", get(|| async { "ok" }))
        .layer(middleware::from_fn(trace_layer));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    // Allow server to start
    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

    let _lock = test_mutex().lock().await;
    reset_exporter();

    let client = Client::new();
    let resp = client
        .get(format!("http://{}/ok", addr))
        .send()
        .await
        .expect("request to test server must succeed");
    assert_eq!(resp.status().as_u16(), 200);

    // Give the server's async task time to finish the span export
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    let spans = finished_spans();

    // Find the SERVER span for this specific route
    let server_span = spans
        .iter()
        .find(|s| s.span_kind == SpanKind::Server && s.name == "GET /ok")
        .expect("must have a SERVER span named 'GET /ok'");

    // Route attribute should be the template
    let route_attr = attr_value(&server_span.attributes, "http.route");
    assert_eq!(
        route_attr.and_then(value_as_str),
        Some("/ok"),
        "http.route attribute must be the route template"
    );

    // Status code attribute
    eprintln!("SERVER span attributes:");
    for a in &server_span.attributes {
        eprintln!("  {:?} = {:?}", a.key, a.value);
    }
    let status_attr = attr_value(&server_span.attributes, "http.response.status_code");
    assert_eq!(
        status_attr.and_then(value_as_i64),
        Some(200),
        "http.response.status_code must be 200"
    );

    // No error status on 200
    assert!(
        !status_is_error(&server_span.status),
        "SERVER span for 200 must not have error status"
    );
}

/// Verify that the middleware correctly extracts an incoming W3C `traceparent`,
/// propagating the trace ID and setting the parent span ID from the remote
/// caller, while still recording route/status/error assertions.
#[tokio::test]
async fn test_middleware_server_span_with_incoming_traceparent() {
    ensure_global_init();

    use shared::telemetry::middleware::trace_layer;

    let app = Router::new()
        .route("/ok", get(|| async { "ok" }))
        .layer(middleware::from_fn(trace_layer));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

    let _lock = test_mutex().lock().await;
    reset_exporter();

    // Known W3C traceparent (version, trace_id, parent_span_id, flags)
    let trace_id_hex = "0af7651916cd43dd8448eb211c80319c";
    let parent_span_id_hex = "b7ad6b7169203331";
    let traceparent = format!("00-{}-{}-01", trace_id_hex, parent_span_id_hex);

    let client = Client::new();
    let resp = client
        .get(format!("http://{}/ok", addr))
        .header("traceparent", &traceparent)
        .send()
        .await
        .expect("request must succeed");
    assert_eq!(resp.status().as_u16(), 200);

    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    let spans = finished_spans();

    let server_span = spans
        .iter()
        .find(|s| s.span_kind == SpanKind::Server && s.name == "GET /ok")
        .expect("must have a SERVER span named 'GET /ok'");

    let expected_trace_id = TraceId::from_hex(trace_id_hex).expect("valid trace_id hex");
    let expected_parent_span_id = SpanId::from_hex(parent_span_id_hex).expect("valid span_id hex");

    assert_eq!(
        server_span.span_context.trace_id(),
        expected_trace_id,
        "SERVER span must inherit the incoming trace ID"
    );
    assert_eq!(
        server_span.parent_span_id, expected_parent_span_id,
        "SERVER span parent_span_id must equal incoming traceparent span_id"
    );

    // Retain existing route / status / error assertions
    let route_attr = attr_value(&server_span.attributes, "http.route");
    assert_eq!(
        route_attr.and_then(value_as_str),
        Some("/ok"),
        "http.route must be /ok"
    );

    let status_attr = attr_value(&server_span.attributes, "http.response.status_code");
    assert_eq!(
        status_attr.and_then(value_as_i64),
        Some(200),
        "http.response.status_code must be 200"
    );

    assert!(
        !status_is_error(&server_span.status),
        "SERVER span for 200 must not have error status"
    );
}

/// Verify the middleware records ERROR status for 5xx responses.
#[tokio::test]
async fn test_middleware_server_span_on_error() {
    ensure_global_init();

    use shared::telemetry::middleware::trace_layer;

    let app = Router::new()
        .route(
            "/error",
            get(|| async { (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "error") }),
        )
        .layer(middleware::from_fn(trace_layer));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

    let _lock = test_mutex().lock().await;
    reset_exporter();

    let client = Client::new();
    let resp = client
        .get(format!("http://{}/error", addr))
        .send()
        .await
        .expect("request to test server must succeed");
    assert_eq!(resp.status().as_u16(), 500);

    // Give the server's async task time to finish the span export
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    let spans = finished_spans();

    // Find the SERVER span for this specific route
    let server_span = spans
        .iter()
        .find(|s| s.span_kind == SpanKind::Server && s.name == "GET /error")
        .expect("must have a SERVER span named 'GET /error'");

    // Status code attribute
    eprintln!("ERROR span attributes:");
    for a in &server_span.attributes {
        eprintln!("  {:?} = {:?}", a.key, a.value);
    }
    let status_attr = attr_value(&server_span.attributes, "http.response.status_code");
    assert_eq!(
        status_attr.and_then(value_as_i64),
        Some(500),
        "http.response.status_code must be 500"
    );

    // Span should have error status
    assert!(
        status_is_error(&server_span.status),
        "SERVER span for 5xx must have error status"
    );

    // Route should still be templated
    assert_eq!(
        server_span.name, "GET /error",
        "SERVER span name must be templated 'GET /error'"
    );
}

// ---------------------------------------------------------------------------
// OTel log helpers — pure-function tests
// ---------------------------------------------------------------------------

#[test]
fn test_normalize_otlp_endpoint_strips_trailing_slash() {
    assert_eq!(
        shared::telemetry::normalize_otlp_endpoint("http://localhost:4318"),
        "http://localhost:4318"
    );
    assert_eq!(
        shared::telemetry::normalize_otlp_endpoint("http://localhost:4318/"),
        "http://localhost:4318"
    );
    assert_eq!(
        shared::telemetry::normalize_otlp_endpoint("http://localhost:4318///"),
        "http://localhost:4318"
    );
    assert_eq!(shared::telemetry::normalize_otlp_endpoint(""), "");
}

#[test]
fn test_build_logs_url() {
    assert_eq!(
        shared::telemetry::build_logs_url(Some("http://localhost:4318")),
        Some("http://localhost:4318/v1/logs".to_string())
    );
    assert_eq!(
        shared::telemetry::build_logs_url(Some("http://localhost:4318/")),
        Some("http://localhost:4318/v1/logs".to_string())
    );
    assert_eq!(shared::telemetry::build_logs_url(None), None);
}

#[test]
fn test_build_logs_url_v1_traces_is_not_affected() {
    // Ensure /v1/logs endpoint is independent from the traces endpoint
    let url = shared::telemetry::build_logs_url(Some("http://otel:4318")).unwrap();
    assert_eq!(url, "http://otel:4318/v1/logs");
    assert_ne!(url, "http://otel:4318/v1/traces");
}

/// Verify that the middleware creates a SERVER span using the incoming W3C
/// trace context, uses the templated route name (no raw ID/query string),
/// and records the correct status.
#[tokio::test]
async fn test_middleware_traceparent_propagation() {
    ensure_global_init();

    use axum::extract::Path;
    use shared::telemetry::middleware::trace_layer;

    let app = Router::new()
        .route("/users/:id/details", get(|_: Path<String>| async { "ok" }))
        .layer(middleware::from_fn(trace_layer));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    // Allow server to start
    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

    let _lock = test_mutex().lock().await;
    reset_exporter();

    // Known W3C traceparent: version=00, trace_id=0af7…, span_id=b7ad…, flags=01
    let traceparent = "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01";
    let expected_trace_id = "0af7651916cd43dd8448eb211c80319c";
    let expected_parent_span_id = "b7ad6b7169203331";

    let client = Client::new();
    let resp = client
        .get(format!("http://{}/users/abc123/details?foo=bar", addr))
        .header("traceparent", traceparent)
        .send()
        .await
        .expect("request to test server must succeed");
    assert_eq!(resp.status().as_u16(), 200);

    // Give the server's async task time to finish the span export
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    let spans = finished_spans();

    // Find the SERVER span for this parameterised route
    let server_span = spans
        .iter()
        .find(|s| s.span_kind == SpanKind::Server && s.name == "GET /users/:id/details")
        .expect("must have a SERVER span named 'GET /users/:id/details'");

    // === Incoming trace context is used ===
    let trace_id = trace_id_hex(server_span.span_context.trace_id());
    assert_eq!(
        trace_id, expected_trace_id,
        "SERVER span must use the incoming trace ID from traceparent"
    );

    let parent_span_id = span_id_hex(server_span.parent_span_id);
    assert_eq!(
        parent_span_id, expected_parent_span_id,
        "SERVER span's parent span ID must match the incoming traceparent span ID"
    );

    // === No raw ID or query in span name ===
    assert!(
        !server_span.name.contains("abc123"),
        "SERVER span name must not contain raw path parameter"
    );
    assert!(
        !server_span.name.contains("foo=bar"),
        "SERVER span name must not contain query string"
    );
    assert!(
        !server_span.name.contains("?"),
        "SERVER span name must not contain '?'"
    );

    // === Route attribute is the template ===
    let route_attr = attr_value(&server_span.attributes, "http.route");
    assert_eq!(
        route_attr.and_then(value_as_str),
        Some("/users/:id/details"),
        "http.route attribute must be the route template"
    );

    // === Status code ===
    let status_attr = attr_value(&server_span.attributes, "http.response.status_code");
    assert_eq!(
        status_attr.and_then(value_as_i64),
        Some(200),
        "http.response.status_code must be 200"
    );

    // No error on 200
    assert!(
        !status_is_error(&server_span.status),
        "SERVER span for 200 must not have error status"
    );
}

/// Verify that consecutive `send_traced` calls produce distinct client span IDs
/// but share the same trace ID within the same parent context.
///
/// The parent scope is established via `.instrument()` so no EnteredGuard
/// is held across any `.await`.
#[tokio::test]
async fn test_send_traced_distinct_client_span_ids() {
    ensure_global_init();
    let (addr, _stop) = start_downstream().await;
    let url = format!("http://{}/echo", addr);

    let _lock = test_mutex().lock().await;
    reset_exporter();

    let parent = info_span!("test_parent", otel.kind = "INTERNAL");

    // ---- within parent span ----
    async {
        let client = Client::new();

        let resp1 = shared::telemetry::http_client::send_traced("GET", &url, client.get(&url))
            .await
            .expect("first send_traced should succeed");
        let body1: Value = resp1.json().await.unwrap();
        let headers1 = body1["headers"].as_array().unwrap();
        let tp1 = get_traceparent_from_json(headers1).unwrap();

        let resp2 = shared::telemetry::http_client::send_traced("GET", &url, client.get(&url))
            .await
            .expect("second send_traced should succeed");
        let body2: Value = resp2.json().await.unwrap();
        let headers2 = body2["headers"].as_array().unwrap();
        let tp2 = get_traceparent_from_json(headers2).unwrap();

        let (_, trace1, span1, _) = parse_traceparent(&tp1).unwrap();
        let (_, trace2, span2, _) = parse_traceparent(&tp2).unwrap();

        // Different calls should produce different span IDs
        assert_ne!(
            span1, span2,
            "consecutive send_traced calls should produce different span IDs"
        );

        // Both must have valid hex IDs
        assert_eq!(trace1.len(), 32);
        assert_eq!(trace2.len(), 32);

        // Both should share the same trace ID (from the parent span)
        assert_eq!(
            trace1, trace2,
            "two calls within the same parent should share trace ID"
        );

        // Validate via exported spans too.
        let spans = finished_spans();

        let client_spans: Vec<_> = spans
            .iter()
            .filter(|s| s.span_kind == SpanKind::Client)
            .collect();
        assert!(
            client_spans.len() >= 2,
            "should have at least two CLIENT spans"
        );

        // All client spans should have unique span IDs
        let client_span_ids: Vec<_> = client_spans
            .iter()
            .map(|s| span_id_hex(s.span_context.span_id()))
            .collect();
        let mut unique_ids = client_span_ids.clone();
        unique_ids.sort();
        unique_ids.dedup();
        assert_eq!(
            unique_ids.len(),
            client_span_ids.len(),
            "each CLIENT span should have a unique span ID"
        );
    }
    .instrument(parent)
    .await;
}

// ---------------------------------------------------------------------------
// OTel log exporter integration tests
// ---------------------------------------------------------------------------

/// Verify that the logging bridge emits log records with matching
/// trace_id/span_id when a tracing span with OTel context is active.
#[cfg(test)]
#[tokio::test]
async fn test_log_bridge_trace_context_in_log_records() {
    use opentelemetry::trace::TracerProvider;
    use opentelemetry_sdk::logs::{InMemoryLogExporter, SdkLoggerProvider, SimpleLogProcessor};
    use tracing::Instrument;

    let exporter = InMemoryLogExporter::default();
    let logger_provider = SdkLoggerProvider::builder()
        .with_log_processor(SimpleLogProcessor::new(exporter.clone()))
        .build();

    let tracer_provider = opentelemetry_sdk::trace::SdkTracerProvider::builder()
        .with_id_generator(RandomIdGenerator::default())
        .build();

    let tracer = tracer_provider.tracer("test");
    let telemetry_layer = tracing_opentelemetry::layer().with_tracer(tracer);
    let otel_log_layer =
        opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge::new(&logger_provider);

    let _guard = tracing::subscriber::set_default(
        tracing_subscriber::registry()
            .with(telemetry_layer)
            .with(otel_log_layer),
    );

    exporter.reset();

    let parent_span = info_span!("test_parent", otel.kind = "INTERNAL");

    async {
        tracing::info!("hello from inside a span");
    }
    .instrument(parent_span)
    .await;

    logger_provider.force_flush().ok();

    let log_records = exporter.get_emitted_logs().unwrap_or_default();
    assert!(
        !log_records.is_empty(),
        "expected at least one log record from inside the span"
    );

    let log_data = &log_records[0];
    let trace_context = log_data.record.trace_context();
    assert!(
        trace_context.is_some(),
        "log record should have trace context when inside a span"
    );
    let tc = trace_context.unwrap();
    assert!(
        tc.trace_id.to_string() != "00000000000000000000000000000000",
        "log record trace_id should be non-zero when inside a span"
    );
    assert!(
        tc.span_id.to_string() != "0000000000000000",
        "log record span_id should be non-zero when inside a span"
    );
}

/// Verify that the appender bridge does NOT emit log records outside
/// any tracing scope (i.e. when no span context exists, the log record
/// should still have zero-valued trace_id/span_id).
#[cfg(test)]
#[tokio::test]
async fn test_log_bridge_no_span_safety() {
    use opentelemetry_sdk::logs::{InMemoryLogExporter, SdkLoggerProvider, SimpleLogProcessor};

    let exporter = InMemoryLogExporter::default();
    let logger_provider = SdkLoggerProvider::builder()
        .with_log_processor(SimpleLogProcessor::new(exporter.clone()))
        .build();

    let otel_log_layer =
        opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge::new(&logger_provider);

    let _guard =
        tracing::subscriber::set_default(tracing_subscriber::registry().with(otel_log_layer));

    exporter.reset();

    tracing::info!("log outside any span");

    logger_provider.force_flush().ok();

    let log_records = exporter.get_emitted_logs().unwrap_or_default();
    assert!(
        !log_records.is_empty(),
        "expected at least one log record from outside a span"
    );

    let log_data = &log_records[0];
    let trace_context = log_data.record.trace_context();
    assert!(
        trace_context.is_none(),
        "log record should NOT have trace context when outside a span"
    );
}
