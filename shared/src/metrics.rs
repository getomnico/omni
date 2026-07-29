//! Centralised OpenTelemetry metric instruments for Omni application services.
//!
//! Every instrument uses the `omni.*` namespace and is created lazily from
//! the global meter provider.  Callers just invoke the free functions defined
//! here — they are safe to call from any thread and return cached instruments.
//!
//! # Conventions
//!
//! - **Counters** use `u64` and [`Counter::add`].
//! - **Histograms** use `f64` seconds and [`Histogram::record`].
//! - **Gauges** are created ad-hoc with a meter; callers that need frequent
//!   gauge snapshots should define a cached static.

use std::sync::LazyLock;

use opentelemetry::{
    global,
    metrics::{Counter, Gauge, Histogram},
    KeyValue,
};

// ---------------------------------------------------------------------------
// HTTP server RED metrics (cross-cutting, used by all HTTP services)
// ---------------------------------------------------------------------------

/// HTTP server request counter with bounded attributes (method, route, status).
pub static HTTP_SERVER_REQUEST_COUNT: LazyLock<Counter<u64>> = LazyLock::new(|| {
    global::meter("omni")
        .u64_counter("omni.http.server.request_count")
        .with_description("Total number of HTTP server requests by method, route, status")
        .build()
});

/// HTTP server request duration histogram in seconds.
pub static HTTP_SERVER_REQUEST_DURATION: LazyLock<Histogram<f64>> = LazyLock::new(|| {
    global::meter("omni")
        .f64_histogram("omni.http.server.request_duration_seconds")
        .with_description("HTTP server request duration in seconds")
        .with_unit("s")
        .build()
});

/// Record an HTTP server request for RED metrics.
///
/// Attributes are limited to bounded values: method (e.g. GET, POST),
/// route (matched path template), and numeric status code.
pub fn record_http_request(method: &str, route: Option<&str>, status: u16, duration_secs: f64) {
    let mut attrs = vec![
        KeyValue::new("http.request.method", method.to_string()),
        KeyValue::new("http.response.status_code", status as i64),
    ];
    if let Some(r) = route {
        attrs.push(KeyValue::new("http.route", r.to_string()));
    }
    HTTP_SERVER_REQUEST_COUNT.add(1, &attrs);
    HTTP_SERVER_REQUEST_DURATION.record(duration_secs, &attrs);
}

// ---------------------------------------------------------------------------
// Indexer metrics
// ---------------------------------------------------------------------------

/// Total connector events processed (by outcome: processed | failed | dead_letter).
pub static INDEXER_EVENTS_PROCESSED: LazyLock<Counter<u64>> = LazyLock::new(|| {
    global::meter("omni-indexer")
        .u64_counter("omni.indexer.events.processed")
        .with_description("Total connector events successfully processed")
        .build()
});

pub static INDEXER_EVENTS_DEAD_LETTER: LazyLock<Counter<u64>> = LazyLock::new(|| {
    global::meter("omni-indexer")
        .u64_counter("omni.indexer.events.dead_letter")
        .with_description("Total connector events sent to dead-letter queue")
        .build()
});

pub static INDEXER_EVENTS_RETRIED: LazyLock<Counter<u64>> = LazyLock::new(|| {
    global::meter("omni-indexer")
        .u64_counter("omni.indexer.events.retried")
        .with_description("Total connector events retried")
        .build()
});

pub static INDEXER_BATCH_DURATION: LazyLock<Histogram<f64>> = LazyLock::new(|| {
    global::meter("omni-indexer")
        .f64_histogram("omni.indexer.batch.duration_seconds")
        .with_description("Duration of event batch processing in seconds")
        .with_unit("s")
        .build()
});

pub static INDEXER_BATCH_SIZE: LazyLock<Histogram<u64>> = LazyLock::new(|| {
    global::meter("omni-indexer")
        .u64_histogram("omni.indexer.batch.size")
        .with_description("Number of events in a processed batch")
        .with_unit("{events}")
        .build()
});

// ---------------------------------------------------------------------------
// Queue depth gauge (cached static, used by indexer heartbeat)
// ---------------------------------------------------------------------------

/// Single cached gauge for queue depth with bounded `queue` and `status` attributes.
pub static INDEXER_QUEUE_DEPTH: LazyLock<Gauge<i64>> = LazyLock::new(|| {
    global::meter("omni-indexer")
        .i64_gauge("omni.indexer.queue.depth")
        .with_description("Current depth of event or embedding queue by status")
        .build()
});

/// Record connector-event queue depth snapshot via the cached gauge.
pub fn record_queue_status(pending: i64, processing: i64, failed: i64, dead_letter: i64) {
    let base = [KeyValue::new("queue", "connector_events")];
    INDEXER_QUEUE_DEPTH.record(
        pending,
        &[&base[..], &[KeyValue::new("status", "pending")]].concat(),
    );
    INDEXER_QUEUE_DEPTH.record(
        processing,
        &[&base[..], &[KeyValue::new("status", "processing")]].concat(),
    );
    INDEXER_QUEUE_DEPTH.record(
        failed,
        &[&base[..], &[KeyValue::new("status", "failed")]].concat(),
    );
    INDEXER_QUEUE_DEPTH.record(
        dead_letter,
        &[&base[..], &[KeyValue::new("status", "dead_letter")]].concat(),
    );
}

/// Record embedding queue depth snapshot via the cached gauge.
pub fn record_embedding_queue_status(pending: i64, processing: i64, failed: i64) {
    let base = [KeyValue::new("queue", "embedding")];
    INDEXER_QUEUE_DEPTH.record(
        pending,
        &[&base[..], &[KeyValue::new("status", "pending")]].concat(),
    );
    INDEXER_QUEUE_DEPTH.record(
        processing,
        &[&base[..], &[KeyValue::new("status", "processing")]].concat(),
    );
    INDEXER_QUEUE_DEPTH.record(
        failed,
        &[&base[..], &[KeyValue::new("status", "failed")]].concat(),
    );
}
// ---------------------------------------------------------------------------
// Searcher metrics
// ---------------------------------------------------------------------------

pub static SEARCHER_SEARCH_DURATION: LazyLock<Histogram<f64>> = LazyLock::new(|| {
    global::meter("omni-searcher")
        .f64_histogram("omni.searcher.search.duration_seconds")
        .with_description("Search request duration in seconds")
        .with_unit("s")
        .build()
});

pub static SEARCHER_SEARCH_RESULTS: LazyLock<Histogram<u64>> = LazyLock::new(|| {
    global::meter("omni-searcher")
        .u64_histogram("omni.searcher.search.results")
        .with_description("Number of results returned by a search request")
        .with_unit("{results}")
        .build()
});

pub static SEARCHER_SEARCH_ERRORS: LazyLock<Counter<u64>> = LazyLock::new(|| {
    global::meter("omni-searcher")
        .u64_counter("omni.searcher.search.errors")
        .with_description("Total search requests that resulted in an error")
        .build()
});

pub static SEARCHER_CACHE_HIT: LazyLock<Counter<u64>> = LazyLock::new(|| {
    global::meter("omni-searcher")
        .u64_counter("omni.searcher.cache.hit")
        .with_description("Total search requests served from cache")
        .build()
});

pub static SEARCHER_CACHE_MISS: LazyLock<Counter<u64>> = LazyLock::new(|| {
    global::meter("omni-searcher")
        .u64_counter("omni.searcher.cache.miss")
        .with_description("Total search requests that missed cache")
        .build()
});

// ---------------------------------------------------------------------------
// Connector manager sync metrics
// ---------------------------------------------------------------------------

pub static CONNECTOR_SYNC_STARTED: LazyLock<Counter<u64>> = LazyLock::new(|| {
    global::meter("omni-connector-manager")
        .u64_counter("omni.connector.sync.started")
        .with_description("Total sync runs started")
        .build()
});

pub static CONNECTOR_SYNC_COMPLETED: LazyLock<Counter<u64>> = LazyLock::new(|| {
    global::meter("omni-connector-manager")
        .u64_counter("omni.connector.sync.completed")
        .with_description("Total sync runs completed successfully")
        .build()
});

pub static CONNECTOR_SYNC_FAILED: LazyLock<Counter<u64>> = LazyLock::new(|| {
    global::meter("omni-connector-manager")
        .u64_counter("omni.connector.sync.failed")
        .with_description("Total sync runs that failed")
        .build()
});

pub static CONNECTOR_SYNC_CANCELLED: LazyLock<Counter<u64>> = LazyLock::new(|| {
    global::meter("omni-connector-manager")
        .u64_counter("omni.connector.sync.cancelled")
        .with_description("Total sync runs cancelled")
        .build()
});

pub static CONNECTOR_SYNC_DURATION: LazyLock<Histogram<f64>> = LazyLock::new(|| {
    global::meter("omni-connector-manager")
        .f64_histogram("omni.connector.sync.duration_seconds")
        .with_description("Duration of sync runs in seconds")
        .with_unit("s")
        .build()
});

/// Record a terminal sync metric with bounded sync_type and outcome attributes.
/// Never records IDs (sync_run_id, source_id) as attribute values.
/// Duration is included when created_at is available.
pub fn record_sync_terminal(
    sync_type: &str,
    outcome: &str,
    created_at: Option<time::OffsetDateTime>,
) {
    let attrs = [
        KeyValue::new("sync_type", sync_type.to_string()),
        KeyValue::new("outcome", outcome.to_string()),
    ];

    match outcome {
        "completed" => CONNECTOR_SYNC_COMPLETED.add(1, &attrs),
        "failed" => CONNECTOR_SYNC_FAILED.add(1, &attrs),
        "cancelled" => CONNECTOR_SYNC_CANCELLED.add(1, &attrs),
        _ => {}
    }

    if let Some(start_ts) = created_at {
        let now = time::OffsetDateTime::now_utc();
        let duration_secs = (now - start_ts).as_seconds_f64();
        if duration_secs > 0.0 {
            CONNECTOR_SYNC_DURATION.record(duration_secs, &attrs);
        }
    }
}
