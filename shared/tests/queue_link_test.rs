//! Focused integration tests for native OTel link propagation through queues.
//!
//! Uses `InMemorySpanExporter` to inspect exported OpenTelemetry spans and
//! verify that CONSUMER spans carry native links (not string attributes) to
//! their PRODUCER spans.
//!
//! Tests use the *exact production* `build_producer_span` / `inject_active_context`
//! / `collect_span_contexts` helpers from the shared telemetry::queue module.

use std::sync::{Mutex, OnceLock};

use opentelemetry::{
    global,
    trace::{SpanKind, TracerProvider as _},
};
use opentelemetry_sdk::{
    propagation::TraceContextPropagator,
    trace::{InMemorySpanExporter, RandomIdGenerator, SdkTracerProvider, SimpleSpanProcessor},
};
use shared::telemetry::queue;
use tracing::{info_span, Instrument};
use tracing_opentelemetry::OpenTelemetrySpanExt;
use tracing_subscriber::{layer::SubscriberExt as _, util::SubscriberInitExt as _};

// ---------------------------------------------------------------------------
// Global test infrastructure
// ---------------------------------------------------------------------------

/// Process-global mutex to serialise all tests in this file.
/// Every test acquires this guard before accessing the shared exporter.
static TEST_MUTEX: OnceLock<Mutex<()>> = OnceLock::new();
fn test_guard() -> std::sync::MutexGuard<'static, ()> {
    TEST_MUTEX
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|p| p.into_inner())
}

static EXPORTER: OnceLock<InMemorySpanExporter> = OnceLock::new();
fn exporter() -> &'static InMemorySpanExporter {
    EXPORTER.get().expect("exporter not initialised")
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

fn reset_exporter() {
    exporter().reset();
}

fn finished_spans() -> Vec<opentelemetry_sdk::trace::SpanData> {
    exporter()
        .get_finished_spans()
        .expect("get_finished_spans should succeed")
}

/// Find a finished span by name and kind.
fn find_span<'a>(
    spans: &'a [opentelemetry_sdk::trace::SpanData],
    name: &str,
    kind: SpanKind,
) -> Option<&'a opentelemetry_sdk::trace::SpanData> {
    spans.iter().find(|s| s.name == name && s.span_kind == kind)
}

/// Check whether the span has a `messaging.linked_trace_ids` attribute.
fn has_linked_trace_ids_attr(span: &opentelemetry_sdk::trace::SpanData) -> bool {
    span.attributes
        .iter()
        .any(|kv| kv.key.as_str() == "messaging.linked_trace_ids")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Producer and consumer exported; consumer has no parent and a different
/// trace ID; native `links` contains exact producer trace+span ID.
#[test]
fn test_producer_consumer_native_link() {
    let _guard = test_guard();
    ensure_global_init();
    reset_exporter();

    let parent = info_span!("test_root");

    parent.in_scope(|| {
        // ---- Producer: create a PRODUCER span and inject context ----
        let carrier = {
            let prod_span = queue::build_producer_span("test_queue");
            let _g = prod_span.entered();
            queue::inject_active_context()
            // prod_span dropped here with EnteredSpan guard -> exported
        };

        // ---- Consumer: reconstruct context and build CONSUMER span with link ----
        let items = vec![Some((carrier.traceparent, carrier.tracestate))];
        let span_contexts = queue::collect_span_contexts(&items);
        assert_eq!(span_contexts.len(), 1, "should have one producer context");

        let cons_span = info_span!(
            parent: None,
            "test_queue process",
            otel.kind = "CONSUMER",
        );
        {
            let _g = cons_span.clone().entered();
            for sc in &span_contexts {
                cons_span.add_link(sc.clone());
            }
        }
        // Drop cons_span: consumer span ends and is exported.
        drop(cons_span);
    });

    let spans = finished_spans();

    // Find producer and consumer spans
    let producer = find_span(&spans, "test_queue publish", SpanKind::Producer)
        .expect("must have PRODUCER span");
    let consumer = find_span(&spans, "test_queue process", SpanKind::Consumer)
        .expect("must have CONSUMER span");

    // ---- Consumer has no parent (parent_span_id is all zeros) ----
    assert_eq!(
        consumer.parent_span_id,
        opentelemetry::trace::SpanId::INVALID,
        "CONSUMER span must have INVALID parent_span_id (new root)"
    );

    // ---- Consumer has a different trace ID than producer ----
    assert_ne!(
        consumer.span_context.trace_id(),
        producer.span_context.trace_id(),
        "CONSUMER trace ID must differ from PRODUCER trace ID (new root)"
    );

    // ---- Consumer has a native link to the producer ----
    assert!(
        !consumer.links.is_empty(),
        "CONSUMER span must have at least one link"
    );
    let link = &consumer.links[0];
    assert_eq!(
        link.span_context.trace_id(),
        producer.span_context.trace_id(),
        "link trace_id must match PRODUCER trace_id"
    );
    assert_eq!(
        link.span_context.span_id(),
        producer.span_context.span_id(),
        "link span_id must match PRODUCER span_id"
    );

    // ---- No messaging.linked_trace_ids attribute exists ----
    assert!(
        !has_linked_trace_ids_attr(consumer),
        "CONSUMER span must NOT have messaging.linked_trace_ids attribute"
    );
    assert!(
        !has_linked_trace_ids_attr(producer),
        "PRODUCER span must NOT have messaging.linked_trace_ids attribute"
    );
}

/// Two producer spans in the same trace yield two consumer links.
#[test]
fn test_two_producers_same_trace_two_links() {
    let _guard = test_guard();
    ensure_global_init();
    reset_exporter();

    let parent = info_span!("test_root");

    parent.in_scope(|| {
        // First producer
        let c1 = {
            let p1 = queue::build_producer_span("test_queue");
            let _g = p1.entered();
            queue::inject_active_context()
        };

        // Second producer in the same parent trace — same trace_id, different span_id
        let c2 = {
            let p2 = queue::build_producer_span("test_queue");
            let _g = p2.entered();
            queue::inject_active_context()
        };

        // Consumer: collect both contexts
        let items = vec![
            Some((c1.traceparent, c1.tracestate)),
            Some((c2.traceparent, c2.tracestate)),
        ];
        let span_contexts = queue::collect_span_contexts(&items);
        assert_eq!(
            span_contexts.len(),
            2,
            "both producer spans should be retained (different span_id)"
        );

        let cons_span = info_span!(
            parent: None,
            "test_queue process",
            otel.kind = "CONSUMER",
        );
        {
            let _g = cons_span.clone().entered();
            for sc in &span_contexts {
                cons_span.add_link(sc.clone());
            }
        }
        drop(cons_span);
    });

    let spans = finished_spans();

    let consumer = find_span(&spans, "test_queue process", SpanKind::Consumer)
        .expect("must have CONSUMER span");

    assert_eq!(
        consumer.links.len(),
        2,
        "CONSUMER must have 2 links for 2 distinct producer spans"
    );

    // Verify both links have the same trace_id (from the parent)
    let trace_id = consumer.links[0].span_context.trace_id();
    for link in consumer.links.iter() {
        assert_eq!(
            link.span_context.trace_id(),
            trace_id,
            "all links should share the same trace_id"
        );
    }

    // Verify link span IDs are different
    assert_ne!(
        consumer.links[0].span_context.span_id(),
        consumer.links[1].span_context.span_id(),
        "two producer links must have different span IDs"
    );

    assert!(!has_linked_trace_ids_attr(consumer));
}

/// Duplicate same context dedupes to one link.
#[test]
fn test_duplicate_context_dedupes() {
    let _guard = test_guard();
    ensure_global_init();
    reset_exporter();

    let parent = info_span!("test_root");

    parent.in_scope(|| {
        let carrier = {
            let prod_span = queue::build_producer_span("test_queue");
            let _g = prod_span.entered();
            queue::inject_active_context()
        };

        // Duplicate the same carrier
        let items = vec![
            Some((carrier.traceparent.clone(), carrier.tracestate.clone())),
            Some((carrier.traceparent, carrier.tracestate)),
        ];
        let span_contexts = queue::collect_span_contexts(&items);
        assert_eq!(
            span_contexts.len(),
            1,
            "duplicate (trace_id, span_id) must dedupe to 1"
        );
    });
}

/// Invalid/missing items yield zero span contexts.
#[test]
fn test_invalid_missing_yield_zero() {
    let _guard = test_guard();
    ensure_global_init();

    let items: Vec<Option<(Option<String>, Option<String>)>> = vec![
        None,
        Some((None, None)),
        Some((Some("invalid".to_string()), None)),
        Some((
            Some("00-00000000000000000000000000000000-0000000000000000-01".to_string()),
            None,
        )),
    ];
    let span_contexts = queue::collect_span_contexts(&items);
    assert!(
        span_contexts.is_empty(),
        "invalid/missing items should yield zero span contexts"
    );
}

/// Producer span is active during the async operation: verify by entering
/// a span, instrumenting a synchronous block, and checking exports.
#[tokio::test]
async fn test_producer_span_active_during_operation() {
    let _guard = test_guard();
    ensure_global_init();
    reset_exporter();

    let parent = info_span!("test_root");
    let _guard = parent.entered();

    let prod_span = queue::build_producer_span("test_queue");

    async move {
        // Simulate an async operation (e.g. SQL INSERT) inside the producer span.
        // The carrier is acquired inside the instrumented block so the
        // producer span is active during injection and the async work.
        let _carrier = queue::inject_active_context();
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
    }
    .instrument(prod_span)
    .await;

    drop(_guard);

    let spans = finished_spans();

    let producer = find_span(&spans, "test_queue publish", SpanKind::Producer)
        .expect("must have PRODUCER span");

    // Producer span should have messaging.* attributes
    let has_destination = producer
        .attributes
        .iter()
        .any(|kv| kv.key.as_str() == "messaging.destination");
    assert!(
        has_destination,
        "PRODUCER span must have messaging.destination"
    );
}
