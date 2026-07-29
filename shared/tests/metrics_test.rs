//! Integration tests for the shared metrics module.
//!
//! Sets up a single global meter provider backed by `InMemoryMetricExporter`,
//! then invokes the real `shared::metrics` production helpers.  Because the
//! `LazyLock` instruments bind once to the global provider, this must run as
//! a single serialised test.

use std::sync::OnceLock;

use opentelemetry::global;
use opentelemetry_sdk::metrics::{
    data::{AggregatedMetrics, MetricData},
    InMemoryMetricExporter, SdkMeterProvider,
};
use time;
use tokio::sync::Mutex;

static METRICS_MUTEX: OnceLock<Mutex<()>> = OnceLock::new();
fn metrics_mutex() -> &'static Mutex<()> {
    METRICS_MUTEX.get_or_init(|| Mutex::new(()))
}

/// Serialised test that sets up the global meter provider once, invokes every
/// production `shared::metrics` helper, then validates names, units, values,
/// bounded attributes, and seconds-scale durations.
///
/// Because `LazyLock` instruments capture the global provider on first access,
/// all assertions run within a single serialised test.
#[tokio::test]
async fn test_production_metrics_names_and_values() {
    let _lock = metrics_mutex().lock().await;

    // -------------------------------------------------------------------
    // 1. Build an in-memory exporter and install it as the global provider.
    //    This must happen before any shared::metrics static is accessed so
    //    the LazyLock instruments bind to this provider.
    // -------------------------------------------------------------------
    let exporter = InMemoryMetricExporter::default();
    let meter_provider = SdkMeterProvider::builder()
        .with_periodic_exporter(exporter.clone())
        .build();
    global::set_meter_provider(meter_provider.clone());

    // -------------------------------------------------------------------
    // 2. Call every production recording helper.  This triggers LazyLock
    //    evaluation against the provider above.
    // -------------------------------------------------------------------

    // HTTP RED (cross-cutting)
    shared::metrics::record_http_request("GET", Some("/ok"), 200, 0.050);
    shared::metrics::record_http_request("POST", Some("/search"), 201, 0.100);
    shared::metrics::record_http_request("GET", None, 404, 0.010);
    shared::metrics::record_http_request("PUT", Some("/documents/:id"), 400, 0.001);
    shared::metrics::record_http_request("DELETE", Some("/sync/:id/cancel"), 500, 0.500);

    // Indexer
    shared::metrics::INDEXER_EVENTS_PROCESSED.add(5, &[]);
    shared::metrics::INDEXER_EVENTS_PROCESSED.add(3, &[]);
    shared::metrics::INDEXER_EVENTS_DEAD_LETTER.add(2, &[]);
    shared::metrics::INDEXER_EVENTS_RETRIED.add(1, &[]);
    shared::metrics::INDEXER_BATCH_DURATION.record(1.5, &[]);
    shared::metrics::INDEXER_BATCH_SIZE.record(10, &[]);
    shared::metrics::record_queue_status(10, 2, 1, 0);
    shared::metrics::record_embedding_queue_status(5, 1, 0);

    // Searcher
    shared::metrics::SEARCHER_SEARCH_DURATION.record(0.250, &[]);
    shared::metrics::SEARCHER_SEARCH_RESULTS.record(42, &[]);
    shared::metrics::SEARCHER_SEARCH_ERRORS.add(1, &[]);
    shared::metrics::SEARCHER_CACHE_HIT.add(3, &[]);
    shared::metrics::SEARCHER_CACHE_MISS.add(7, &[]);

    // Connector sync
    shared::metrics::CONNECTOR_SYNC_STARTED
        .add(1, &[opentelemetry::KeyValue::new("sync_type", "full")]);
    // Provide created_at so duration is recorded too.
    let now = time::OffsetDateTime::now_utc();
    let five_secs_ago = now - time::Duration::seconds(5);
    shared::metrics::record_sync_terminal("incremental", "completed", Some(five_secs_ago));
    shared::metrics::record_sync_terminal("full", "failed", None);
    shared::metrics::record_sync_terminal("realtime", "cancelled", None);

    // Force flush so all metrics are collected by the exporter.
    meter_provider.force_flush().unwrap();
    let resource_metrics = exporter
        .get_finished_metrics()
        .expect("metrics should be exported after flush");

    assert!(
        !resource_metrics.is_empty(),
        "at least one ResourceMetrics batch expected"
    );

    // -------------------------------------------------------------------
    // 3. Collect all metric names for flexible assertion.
    // -------------------------------------------------------------------
    let mut metric_names: Vec<String> = Vec::new();
    for rm in &resource_metrics {
        for sm in rm.scope_metrics() {
            for m in sm.metrics() {
                metric_names.push(m.name().to_string());
            }
        }
    }

    // -------------------------------------------------------------------
    // 4. Assert expected metric names exist.
    // -------------------------------------------------------------------
    let expected_names = [
        // HTTP RED
        "omni.http.server.request_count",
        "omni.http.server.request_duration_seconds",
        // Indexer
        "omni.indexer.events.processed",
        "omni.indexer.events.dead_letter",
        "omni.indexer.events.retried",
        "omni.indexer.batch.duration_seconds",
        "omni.indexer.batch.size",
        "omni.indexer.queue.depth",
        // Searcher
        "omni.searcher.search.duration_seconds",
        "omni.searcher.search.results",
        "omni.searcher.search.errors",
        "omni.searcher.cache.hit",
        "omni.searcher.cache.miss",
        // Connector
        "omni.connector.sync.started",
        "omni.connector.sync.completed",
        "omni.connector.sync.failed",
        "omni.connector.sync.cancelled",
        "omni.connector.sync.duration_seconds",
    ];

    for name in &expected_names {
        assert!(
            metric_names.contains(&name.to_string()),
            "expected metric '{}' not found. Names: {:?}",
            name,
            metric_names
        );
    }

    // -------------------------------------------------------------------
    // 5. Detailed assertions on specific metrics.
    // -------------------------------------------------------------------
    for rm in &resource_metrics {
        for sm in rm.scope_metrics() {
            for m in sm.metrics() {
                match m.name() {
                    // --- HTTP RED count ---
                    "omni.http.server.request_count" => {
                        assert_counter_total(m, 5, "http.request_count");
                        // Each data point must have numeric status attribute.
                        assert_all_data_points_have_status(m);
                        // Each data point with a route must use template (no raw IDs, no '?').
                        assert_no_raw_path_in_attributes(m);
                    }
                    // --- HTTP RED duration ---
                    "omni.http.server.request_duration_seconds" => {
                        assert_eq!(m.unit(), "s", "unit must be seconds");
                        assert_duration_is_seconds(m, "http.duration_seconds");
                        assert_all_data_points_have_status(m);
                        assert_no_raw_path_in_attributes(m);
                    }
                    // --- Indexer processed ---
                    "omni.indexer.events.processed" => {
                        assert_counter_total(m, 8, "events.processed");
                    }
                    // --- Indexer dead_letter ---
                    "omni.indexer.events.dead_letter" => {
                        assert_counter_total(m, 2, "events.dead_letter");
                    }
                    // --- Indexer retried ---
                    "omni.indexer.events.retried" => {
                        assert_counter_total(m, 1, "events.retried");
                    }
                    // --- Indexer batch duration ---
                    "omni.indexer.batch.duration_seconds" => {
                        assert_eq!(m.unit(), "s", "unit must be seconds");
                        assert_duration_is_seconds(m, "batch.duration");
                    }
                    // --- Indexer batch size ---
                    "omni.indexer.batch.size" => {
                        assert_eq!(m.unit(), "{events}");
                    }
                    // --- Indexer queue depth ---
                    "omni.indexer.queue.depth" => {
                        // Verify status attributes present.
                        assert_queue_depth_has_status_attributes(m);
                    }
                    // --- Searcher duration ---
                    "omni.searcher.search.duration_seconds" => {
                        assert_eq!(m.unit(), "s", "unit must be seconds");
                        assert_duration_is_seconds(m, "search.duration");
                    }
                    // --- Searcher results ---
                    "omni.searcher.search.results" => {
                        assert_eq!(m.unit(), "{results}");
                    }
                    // --- Connector sync started ---
                    "omni.connector.sync.started" => {
                        assert_connector_has_sync_type_attr(m, "started");
                    }
                    // --- Connector sync completed ---
                    "omni.connector.sync.completed" => {
                        assert_connector_has_outcome_attr(m, "completed", "completed");
                    }
                    // --- Connector sync failed ---
                    "omni.connector.sync.failed" => {
                        assert_connector_has_outcome_attr(m, "failed", "failed");
                    }
                    // --- Connector sync cancelled ---
                    "omni.connector.sync.cancelled" => {
                        assert_connector_has_outcome_attr(m, "cancelled", "cancelled");
                    }
                    // --- Connector sync duration ---
                    "omni.connector.sync.duration_seconds" => {
                        assert_eq!(m.unit(), "s", "unit must be seconds");
                    }
                    _ => {}
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Assertion helpers
// ---------------------------------------------------------------------------

fn assert_counter_total(m: &opentelemetry_sdk::metrics::data::Metric, expected: u64, label: &str) {
    match m.data() {
        AggregatedMetrics::U64(data) => {
            if let MetricData::Sum(s) = data {
                let total: u64 = s.data_points().map(|dp| dp.value()).sum();
                assert_eq!(total, expected, "{label}: expected {expected}, got {total}");
            }
        }
        _ => panic!("{label}: expected U64 counter"),
    }
}

fn assert_duration_is_seconds(m: &opentelemetry_sdk::metrics::data::Metric, label: &str) {
    match m.data() {
        AggregatedMetrics::F64(data) => {
            if let MetricData::Histogram(h) = data {
                for dp in h.data_points() {
                    if let Some(v) = dp.min() {
                        assert!(
                            v < 10.0,
                            "{label}: duration must be seconds, not ms (got {v})"
                        );
                    }
                }
            }
        }
        _ => panic!("{label}: expected F64 histogram"),
    }
}

fn assert_all_data_points_have_status(m: &opentelemetry_sdk::metrics::data::Metric) {
    match m.data() {
        AggregatedMetrics::U64(data) => {
            if let MetricData::Sum(s) = data {
                for dp in s.data_points() {
                    assert!(
                        dp.attributes()
                            .any(|a| a.key.as_str() == "http.response.status_code"),
                        "http.response.status_code attribute required"
                    );
                    // Status must be numeric (i64).
                    for a in dp.attributes() {
                        if a.key.as_str() == "http.response.status_code" {
                            let _: i64 = match &a.value {
                                opentelemetry::Value::I64(v) => *v,
                                _ => panic!("status_code must be I64, got {:?}", a.value),
                            };
                        }
                    }
                }
            }
        }
        AggregatedMetrics::F64(data) => {
            if let MetricData::Histogram(h) = data {
                for dp in h.data_points() {
                    assert!(
                        dp.attributes()
                            .any(|a| a.key.as_str() == "http.response.status_code"),
                        "http.response.status_code attribute required"
                    );
                }
            }
        }
        _ => {}
    }
}

fn assert_no_raw_path_in_attributes(m: &opentelemetry_sdk::metrics::data::Metric) {
    let check = |a: &opentelemetry::KeyValue| {
        let s = a.value.as_str().to_string();
        assert!(!s.contains('?'), "attributes must not contain '?': {s}");
    };
    match m.data() {
        AggregatedMetrics::U64(data) => {
            if let MetricData::Sum(s) = data {
                for dp in s.data_points() {
                    for a in dp.attributes() {
                        check(&a);
                    }
                }
            }
        }
        AggregatedMetrics::F64(data) => {
            if let MetricData::Histogram(h) = data {
                for dp in h.data_points() {
                    for a in dp.attributes() {
                        check(&a);
                    }
                }
            }
        }
        _ => {}
    }
}

fn assert_queue_depth_has_status_attributes(m: &opentelemetry_sdk::metrics::data::Metric) {
    match m.data() {
        AggregatedMetrics::I64(data) => {
            if let MetricData::Gauge(g) = data {
                let statuses: Vec<String> = g
                    .data_points()
                    .filter_map(|dp| {
                        dp.attributes()
                            .find(|a| a.key.as_str() == "status")
                            .map(|a| a.value.as_str().to_string())
                    })
                    .collect();
                // At minimum we should have pending and dead_letter statuses.
                assert!(
                    statuses.contains(&"pending".to_string()),
                    "queue.depth must have 'pending' status, got {:?}",
                    statuses
                );
                assert!(
                    statuses.contains(&"dead_letter".to_string()),
                    "queue.depth must have 'dead_letter' status, got {:?}",
                    statuses
                );
            }
        }
        _ => panic!("queue.depth expected I64 gauge"),
    }
}

fn assert_connector_has_sync_type_attr(m: &opentelemetry_sdk::metrics::data::Metric, label: &str) {
    match m.data() {
        AggregatedMetrics::U64(data) => {
            if let MetricData::Sum(s) = data {
                for dp in s.data_points() {
                    let has_sync_type = dp.attributes().any(|a| a.key.as_str() == "sync_type");
                    assert!(
                        has_sync_type,
                        "{label}: sync_type attribute required on started"
                    );
                    // No IDs.
                    assert!(
                        !dp.attributes().any(|a| a.key.as_str() == "sync_run_id"),
                        "{label}: must not contain sync_run_id"
                    );
                    assert!(
                        !dp.attributes().any(|a| a.key.as_str() == "source_id"),
                        "{label}: must not contain source_id"
                    );
                }
            }
        }
        _ => panic!("{label}: expected U64 counter"),
    }
}

fn assert_connector_has_outcome_attr(
    m: &opentelemetry_sdk::metrics::data::Metric,
    label: &str,
    expected_outcome: &str,
) {
    match m.data() {
        AggregatedMetrics::U64(data) => {
            if let MetricData::Sum(s) = data {
                for dp in s.data_points() {
                    let outcome = dp.attributes().find(|a| a.key.as_str() == "outcome");
                    assert!(outcome.is_some(), "{label}: outcome attribute required");
                    let val = outcome.unwrap().value.as_str().to_string();
                    assert_eq!(val, expected_outcome, "{label}: outcome mismatch");
                }
            }
        }
        _ => panic!("{label}: expected U64 counter"),
    }
}
