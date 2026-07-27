use anyhow::Result;
use opentelemetry::{global, trace::TracerProvider as _, KeyValue};
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::{
    logs::{
        log_processor_with_async_runtime::BatchLogProcessor as AsyncBatchLogProcessor,
        SdkLoggerProvider,
    },
    metrics::{PeriodicReader, SdkMeterProvider},
    propagation::TraceContextPropagator,
    runtime,
    trace::{
        span_processor_with_async_runtime::BatchSpanProcessor, RandomIdGenerator, Sampler,
        SdkTracerProvider,
    },
    Resource,
};
use opentelemetry_semantic_conventions::{
    resource::{SERVICE_NAME, SERVICE_VERSION},
    SCHEMA_URL,
};
use std::{sync::OnceLock, time::Duration};
use tracing::warn;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, Layer};

static TRACER_PROVIDER: OnceLock<SdkTracerProvider> = OnceLock::new();
static METER_PROVIDER: OnceLock<SdkMeterProvider> = OnceLock::new();
static LOGGER_PROVIDER: OnceLock<SdkLoggerProvider> = OnceLock::new();

pub struct TelemetryConfig {
    pub service_name: String,
    pub otlp_endpoint: Option<String>,
    pub deployment_id: String,
    pub environment: String,
    pub service_version: String,
}

impl TelemetryConfig {
    pub fn from_env(service_name: &str) -> Self {
        Self {
            service_name: service_name.to_string(),
            otlp_endpoint: std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT")
                .ok()
                .filter(|s| !s.is_empty()),
            deployment_id: std::env::var("OTEL_DEPLOYMENT_ID")
                .unwrap_or_else(|_| ulid::Ulid::new().to_string()),
            environment: std::env::var("OTEL_DEPLOYMENT_ENVIRONMENT")
                .unwrap_or_else(|_| "development".to_string()),
            service_version: std::env::var("SERVICE_VERSION")
                .unwrap_or_else(|_| env!("CARGO_PKG_VERSION").to_string()),
        }
    }
}

/// Normalise an OTLP endpoint: strip trailing slash.
/// Shared pure helper exposed for testing.
pub fn normalize_otlp_endpoint(endpoint: &str) -> String {
    endpoint.trim_end_matches('/').to_string()
}

/// Build the full OTLP HTTP log export URL from a base endpoint.
/// Returns `None` when the base endpoint is absent.
pub fn build_logs_url(endpoint: Option<&str>) -> Option<String> {
    endpoint.map(|e| format!("{}/v1/logs", normalize_otlp_endpoint(e)))
}

pub fn init_telemetry(config: TelemetryConfig) -> Result<()> {
    // Configure W3C TraceContext propagator for trace propagation
    global::set_text_map_propagator(TraceContextPropagator::new());

    let resource = Resource::builder_empty()
        .with_schema_url(
            [
                KeyValue::new(SERVICE_NAME, config.service_name.clone()),
                KeyValue::new(SERVICE_VERSION, config.service_version.clone()),
                KeyValue::new("deployment.environment", config.environment.clone()),
                KeyValue::new("deployment.id", config.deployment_id.clone()),
            ],
            SCHEMA_URL,
        )
        .build();

    let otlp_endpoint_for_log = config.otlp_endpoint.clone();

    let endpoint = config
        .otlp_endpoint
        .map(|e| e.trim_end_matches('/').to_string());

    let tracer_provider = if let Some(ref endpoint) = endpoint {
        let traces_url = format!("{}/v1/traces", endpoint);
        let exporter = opentelemetry_otlp::SpanExporter::builder()
            .with_http()
            .with_endpoint(traces_url)
            .with_timeout(Duration::from_secs(10))
            .build()?;

        let batch = BatchSpanProcessor::builder(exporter, runtime::Tokio).build();
        SdkTracerProvider::builder()
            .with_span_processor(batch)
            .with_resource(resource.clone())
            .with_sampler(Sampler::AlwaysOn)
            .with_id_generator(RandomIdGenerator::default())
            .build()
    } else {
        SdkTracerProvider::builder()
            .with_resource(resource.clone())
            .with_sampler(Sampler::AlwaysOn)
            .with_id_generator(RandomIdGenerator::default())
            .build()
    };

    global::set_tracer_provider(tracer_provider.clone());
    let _ = TRACER_PROVIDER.set(tracer_provider.clone());

    // ------------------------------------------------------------------
    // Meter provider (metrics)
    // ------------------------------------------------------------------
    let meter_provider = if let Some(ref endpoint) = endpoint {
        let metrics_url = format!("{}/v1/metrics", endpoint);
        let metric_exporter = opentelemetry_otlp::MetricExporter::builder()
            .with_http()
            .with_endpoint(metrics_url)
            .with_timeout(Duration::from_secs(10))
            .build()?;

        // Read OTEL_METRIC_EXPORT_INTERVAL as milliseconds, default 60000.
        let metric_export_interval_ms = std::env::var("OTEL_METRIC_EXPORT_INTERVAL")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .filter(|&v| v > 0)
            .unwrap_or(60000);

        let reader = PeriodicReader::builder(metric_exporter)
            .with_interval(Duration::from_millis(metric_export_interval_ms))
            .build();

        SdkMeterProvider::builder()
            .with_resource(resource.clone())
            .with_reader(reader)
            .build()
    } else {
        // No endpoint configured — build a provider that collects metrics
        // but drops them.  Instruments will still be callable (no crash)
        // but data goes nowhere.
        SdkMeterProvider::builder()
            .with_resource(resource.clone())
            .build()
    };

    global::set_meter_provider(meter_provider.clone());
    let _ = METER_PROVIDER.set(meter_provider.clone());

    // ------------------------------------------------------------------
    // Logger provider (logs)
    // ------------------------------------------------------------------
    let logger_provider = if let Some(ref endpoint) = endpoint.as_deref() {
        let logs_url = format!("{}/v1/logs", endpoint);
        let log_exporter = opentelemetry_otlp::LogExporter::builder()
            .with_http()
            .with_endpoint(logs_url)
            .with_timeout(Duration::from_secs(10))
            .build()?;

        SdkLoggerProvider::builder()
            .with_resource(resource.clone())
            .with_log_processor(
                AsyncBatchLogProcessor::builder(log_exporter, runtime::Tokio).build(),
            )
            .build()
    } else {
        SdkLoggerProvider::builder()
            .with_resource(resource.clone())
            .build()
    };

    let _ = LOGGER_PROVIDER.set(logger_provider.clone());

    // ------------------------------------------------------------------
    // Tracing layers
    // ------------------------------------------------------------------
    let tracer = tracer_provider.tracer(config.service_name.clone());
    let telemetry_layer = tracing_opentelemetry::layer().with_tracer(tracer);

    // Bridge tracing events to OTel log records when a LoggerProvider
    // with OTLP export is active.
    let otel_log_layer =
        opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge::new(&logger_provider);

    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info"))
        .add_directive("sqlx=warn".parse()?)
        .add_directive("hyper=info".parse()?)
        .add_directive("tower_http=info".parse()?);

    // JSON fmt layer with current-span fields so trace_id/span_id
    // (recorded on SERVER/CLIENT spans by the middleware after
    // OTel context creation) appear in stdout JSON log lines.
    use tracing_subscriber::fmt::format::Format;
    let fmt = Format::default()
        .json()
        .with_current_span(true)
        .with_span_list(true);
    let fmt_layer = tracing_subscriber::fmt::layer()
        .event_format(fmt)
        .with_target(true)
        .with_level(true)
        .with_thread_ids(false)
        .with_file(false)
        .with_line_number(false)
        .with_filter(env_filter);

    tracing_subscriber::registry()
        .with(telemetry_layer)
        .with(otel_log_layer)
        .with(fmt_layer)
        .init();

    tracing::info!(
        service_name = %config.service_name,
        deployment_id = %config.deployment_id,
        environment = %config.environment,
        otlp_endpoint = ?otlp_endpoint_for_log,
        "Telemetry initialized (traces + metrics + logs)"
    );

    Ok(())
}

pub async fn shutdown_telemetry() {
    tracing::info!("Shutting down telemetry");

    // Shut down meter provider first (flush any pending metric exports).
    // This is safe to log through because the logger provider is still
    // active.
    if let Some(meter_provider) = METER_PROVIDER.get() {
        if let Err(error) = meter_provider.shutdown() {
            warn!(%error, "Failed to shut down meter provider");
        }
    }

    // Shut down tracer provider (flush any pending trace exports).
    if let Some(tracer_provider) = TRACER_PROVIDER.get() {
        if let Err(error) = tracer_provider.shutdown() {
            warn!(%error, "Failed to shut down tracer provider");
        }
    }

    // Shut down logger provider last so that final correlated log
    // records (including the shutdown messages above) are exported
    // before the provider is torn down.
    if let Some(logger_provider) = LOGGER_PROVIDER.get() {
        if let Err(error) = logger_provider.shutdown() {
            warn!(%error, "Failed to shut down logger provider");
        }
    }
}

/// Wait for SIGTERM or Ctrl-C. Use this with Axum's
/// `with_graceful_shutdown` to drain connections before
/// shutting down telemetry.
pub async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("Failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("Failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    tracing::info!("Shutdown signal received, starting graceful shutdown");
}

pub mod middleware {
    use crate::metrics;
    use axum::{
        extract::MatchedPath, extract::Request, http::HeaderMap, middleware::Next,
        response::Response,
    };
    use opentelemetry::{global, trace::TraceContextExt};
    use opentelemetry_http::{HeaderExtractor, HeaderInjector};
    use tracing::{Instrument, Span};
    use tracing_opentelemetry::OpenTelemetrySpanExt;

    /// Axum middleware that creates a single OpenTelemetry SERVER span per request
    /// **and** records HTTP RED metrics (request count + duration histogram) via
    /// the central [`metrics::record_http_request`] helper.
    ///
    /// - Extracts W3C TraceContext from incoming headers and sets it as the parent.
    /// - Uses the matched route template (never raw `url.path`) and sets `otel.name`
    ///   to `METHOD /route`.
    /// - Records `http.request.method`, `http.route`, `http.response.status_code`.
    /// - Records `trace_id` / `span_id` onto the span after OTel context creation
    ///   so the JSON formatter (with `with_current_span(true)`) includes them in
    ///   stdout log lines emitted within this request.
    /// - Marks 5xx responses as error spans.
    pub async fn trace_layer(request: Request, response: Next) -> Response {
        let start = std::time::Instant::now();

        let parent_context = global::get_text_map_propagator(|propagator| {
            propagator.extract(&HeaderExtractor(request.headers()))
        });

        let method = request.method().clone();
        let route = request
            .extensions()
            .get::<MatchedPath>()
            .map(|mp| mp.as_str().to_string());

        let otel_name = match &route {
            Some(r) => format!("{} {}", method, r),
            None => format!("{} /unknown", method),
        };

        // Build a single tracing span that tracing-opentelemetry converts into
        // one OTel SERVER span.  The `otel.name` special field overrides the
        // OTel span name; `otel.kind` sets the span kind.
        //
        // `trace_id` and `span_id` are declared as empty fields here.  They are
        // recorded after `set_parent` below once the OTel context exists.
        let span = tracing::info_span!(
            "HTTP request",
            otel.name = otel_name.as_str(),
            otel.kind = "SERVER",
            http.request.method = %method,
            http.route = tracing::field::Empty,
            http.response.status_code = tracing::field::Empty,
            trace_id = tracing::field::Empty,
            span_id = tracing::field::Empty,
        );

        if let Some(ref route_val) = route {
            span.record("http.route", route_val.as_str());
        }

        // Use the extracted parent context
        let _ = span.set_parent(parent_context);

        // Record trace_id and span_id from the (now-set) OTel context onto
        // the tracing span so JSON stdout lines include trace correlation.
        {
            let otel_context = span.context();
            let span_in_context = otel_context.span();
            let sc = span_in_context.span_context();
            if sc.is_valid() {
                span.record("trace_id", sc.trace_id().to_string());
                span.record("span_id", sc.span_id().to_string());
            }
        }

        async move {
            let resp = response.run(request).await;
            let status = resp.status().as_u16() as i64;
            Span::current().record("http.response.status_code", status);

            if status >= 500 {
                Span::current().set_status(opentelemetry::trace::Status::error("Server error"));
            }

            // Record HTTP RED metrics via central helper (bounded attributes only).
            let duration_secs = start.elapsed().as_secs_f64();
            metrics::record_http_request(
                method.as_str(),
                route.as_deref(),
                status as u16,
                duration_secs,
            );

            tracing::info!(status, duration = duration_secs, "Request completed");
            resp
        }
        .instrument(span)
        .await
    }

    /// Inject W3C trace context from the current tracing span into a HeaderMap
    /// (for outbound calls that do not use reqwest).
    pub fn inject_trace_context(headers: &mut HeaderMap) {
        let context = Span::current().context();
        let mut injector = HeaderInjector(headers);
        global::get_text_map_propagator(|propagator| {
            propagator.inject_context(&context, &mut injector);
        });
    }

    /// Extract trace context from request headers and return the trace ID if valid.
    pub fn get_request_id_from_headers(headers: &HeaderMap) -> Option<String> {
        let context = global::get_text_map_propagator(|propagator| {
            propagator.extract(&HeaderExtractor(headers))
        });

        let span = context.span();
        let span_context = span.span_context();
        if span_context.is_valid() {
            Some(span_context.trace_id().to_string())
        } else {
            None
        }
    }
}

pub mod http_client {
    use opentelemetry::global;
    use opentelemetry_http::HeaderInjector;
    use tracing::{Instrument, Span};
    use tracing_opentelemetry::OpenTelemetrySpanExt;

    /// Send an HTTP request wrapped in an OpenTelemetry [`SpanKind::Client`] span.
    ///
    /// The span records `http.request.method`, `server.address`,
    /// `http.response.status_code`, and marks 5xx / transport errors as errors.
    /// W3C trace context is injected using the client span so downstream
    /// services see the correct parent span ID.
    ///
    /// # Arguments
    /// * `method` – HTTP method string (e.g. `"GET"`, `"POST"`)
    /// * `url` – Full request URL (only the host part is recorded as `server.address`)
    /// * `builder` – The pre-configured `reqwest::RequestBuilder`
    ///
    /// The returned future is Send-safe (no EnteredGuard is held across await).
    pub async fn send_traced(
        method: &str,
        url: &str,
        builder: reqwest::RequestBuilder,
    ) -> Result<reqwest::Response, reqwest::Error> {
        let server = url::Url::parse(url)
            .ok()
            .and_then(|u| u.host_str().map(String::from))
            .unwrap_or_else(|| "unknown".to_string());

        let parent_cx = Span::current().context();
        let span_name = format!("HTTP {}", method);

        // Create a CLIENT tracing span; tracing-opentelemetry converts it into
        // an OTel client span with the recorded attributes.
        let span = tracing::info_span!(
            "HTTP client request",
            otel.name = span_name.as_str(),
            otel.kind = "CLIENT",
            http.request.method = %method,
            server.address = %server,
            http.response.status_code = tracing::field::Empty,
        );

        let _ = span.set_parent(parent_cx);

        // Enter the span once so tracing-opentelemetry creates the OTel CLIENT
        // span and stores its OTel context in the tracing span's extensions.
        // The guard is dropped immediately – no EnteredGuard held across await.
        {
            let _guard = span.clone().entered();
        }

        // Retrieve the OTel context from the span's extensions (seeded above).
        // This avoids holding an EnteredGuard across any await point.
        let span_cx = span.context();

        // Inject trace context from the client span so downstream
        // services see this span as the parent.
        let mut headers = http::HeaderMap::new();
        let mut injector = HeaderInjector(&mut headers);
        global::get_text_map_propagator(|propagator| {
            propagator.inject_context(&span_cx, &mut injector);
        });

        let mut builder = builder;
        for (key, value) in headers.iter() {
            builder = builder.header(key.clone(), value.clone());
        }

        // Execute the request inside the span via Instrument so that no
        // EnteredGuard is held across .await.
        async move {
            let result = builder.send().await;

            match &result {
                Ok(response) => {
                    let status = response.status().as_u16() as i64;
                    Span::current().record("http.response.status_code", status);
                    if status >= 500 {
                        Span::current()
                            .set_status(opentelemetry::trace::Status::error("Server error"));
                    }
                }
                Err(e) => {
                    Span::current().set_status(opentelemetry::trace::Status::error(e.to_string()));
                }
            }

            result
        }
        .instrument(span)
        .await
    }
}

/// Queue telemetry helpers for W3C trace context propagation through async queues.
///
/// This module provides:
/// - `inject_producer_context`: creates a PRODUCER span and extracts W3C headers
/// - `extract_remote_span_context`: reconstructs a valid remote SpanContext from stored headers
/// - `add_consumer_links`: adds Link entries to a consumer span from extracted contexts
///
/// Producer spans are parented to the current tracing context (the caller).
/// Consumer spans MUST be created as new roots (no parent) and reference the producer
/// context via links only.
pub mod queue {
    use opentelemetry::{
        global,
        propagation::{Extractor, Injector},
        trace::{SpanContext, SpanId, TraceContextExt, TraceId},
        Context,
    };
    use std::collections::HashSet;
    use tracing::{info_span, Span};
    use tracing_opentelemetry::OpenTelemetrySpanExt;

    const TRACEPARENT_MAX_LEN: usize = 55;
    const TRACESTATE_MAX_LEN: usize = 512;

    /// A text map carrier for W3C trace context.
    #[derive(Debug, Clone, Default)]
    pub struct TraceCarrier {
        pub traceparent: Option<String>,
        pub tracestate: Option<String>,
    }

    impl Injector for TraceCarrier {
        fn set(&mut self, key: &str, value: String) {
            match key.to_lowercase().as_str() {
                "traceparent" => self.traceparent = Some(value),
                "tracestate" => self.tracestate = Some(value),
                _ => {}
            }
        }
    }

    impl Extractor for TraceCarrier {
        fn get(&self, key: &str) -> Option<&str> {
            match key.to_lowercase().as_str() {
                "traceparent" => self.traceparent.as_deref(),
                "tracestate" => self.tracestate.as_deref(),
                _ => None,
            }
        }

        fn keys(&self) -> Vec<&str> {
            let mut keys = Vec::new();
            if self.traceparent.is_some() {
                keys.push("traceparent");
            }
            if self.tracestate.is_some() {
                keys.push("tracestate");
            }
            keys
        }
    }

    /// Build a PRODUCER tracing span for the given queue, parented to the
    /// current scope. The span is returned *un-entered* so the caller can
    /// enter it briefly to inject context before instrumenting the actual
    /// async operation.
    pub fn build_producer_span(queue_name: &str) -> Span {
        let span_name = format!("{} publish", queue_name);
        info_span!(
            target: "queue",
            parent: &Span::current(),
            "{}", span_name,
            otel.name = span_name.as_str(),
            otel.kind = "PRODUCER",
            messaging.system = "postgresql",
            messaging.destination = queue_name,
            messaging.operation.type = "publish",
        )
    }

    /// Inject W3C trace context from the *currently active* tracing span
    /// into a TraceCarrier.  Call this while the producer span is entered.
    pub fn inject_active_context() -> TraceCarrier {
        let cx = Span::current().context();
        let mut carrier = TraceCarrier::default();
        global::get_text_map_propagator(|propagator| propagator.inject_context(&cx, &mut carrier));
        carrier
    }

    /// Extract a valid remote SpanContext from optional stored W3C headers.
    pub fn extract_remote_span_context(
        traceparent: Option<&str>,
        tracestate: Option<&str>,
    ) -> Option<SpanContext> {
        let traceparent = traceparent?;
        if traceparent.len() > TRACEPARENT_MAX_LEN {
            return None;
        }
        let mut carrier = TraceCarrier::default();
        Injector::set(&mut carrier, "traceparent", traceparent.to_string());
        if let Some(ts) = tracestate {
            if ts.len() <= TRACESTATE_MAX_LEN {
                Injector::set(&mut carrier, "tracestate", ts.to_string());
            }
        }
        let cx: Context =
            global::get_text_map_propagator(|p| p.extract_with_context(&Context::new(), &carrier));
        let span = cx.span();
        let sc = span.span_context();
        if sc.is_valid() {
            Some(sc.clone())
        } else {
            None
        }
    }

    /// Collect deduplicated producer SpanContext values from queue items.
    /// Deduplication is by the full `(trace_id, span_id)` pair (not trace
    /// only), so two distinct producer spans within the same trace are
    /// both retained.  Invalid/missing contexts are skipped.
    pub fn collect_span_contexts(
        items: &[Option<(Option<String>, Option<String>)>],
    ) -> Vec<SpanContext> {
        let mut seen: HashSet<(TraceId, SpanId)> = HashSet::new();
        let mut result = Vec::new();
        for ctx in items {
            let (tp, ts) = match ctx {
                Some((tp, ts)) => (tp.as_deref(), ts.as_deref()),
                None => continue,
            };
            let Some(sc) = extract_remote_span_context(tp, ts) else {
                continue;
            };
            let key = (sc.trace_id(), sc.span_id());
            if seen.insert(key) {
                result.push(sc.clone());
            }
        }
        result
    }

    /// Mark the current active span as error with a fixed empty description.
    /// Used to record producer-side failures without leaking message details
    /// into the span status.
    pub fn set_active_span_error() {
        Span::current().set_status(opentelemetry::trace::Status::error(""));
    }

    /// Validate W3C traceparent format.
    pub fn is_valid_traceparent(value: &str) -> bool {
        if value.len() != 55 {
            return false;
        }
        let parts: Vec<&str> = value.split('-').collect();
        parts.len() == 4
            && parts[0] == "00"
            && parts[1].len() == 32
            && parts[1].chars().all(|c| c.is_ascii_hexdigit())
            && parts[2].len() == 16
            && parts[2].chars().all(|c| c.is_ascii_hexdigit())
            && parts[3].len() == 2
            && parts[3].chars().all(|c| c.is_ascii_hexdigit())
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use opentelemetry::trace::TracerProvider as _;
        use opentelemetry_sdk::propagation::TraceContextPropagator;
        use tracing_subscriber::layer::SubscriberExt as _;
        use tracing_subscriber::util::SubscriberInitExt;

        /// Initialize the global TraceContextPropagator once per test run.
        fn init_propagator() {
            use std::sync::Once;
            static INIT: Once = Once::new();
            INIT.call_once(|| {
                global::set_text_map_propagator(TraceContextPropagator::new());
            });
        }

        #[test]
        fn test_is_valid_traceparent() {
            assert!(is_valid_traceparent(
                "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01"
            ));
            assert!(!is_valid_traceparent(
                "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-010"
            ));
            assert!(!is_valid_traceparent("too-short"));
            assert!(!is_valid_traceparent(
                "01-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01"
            ));
            assert!(!is_valid_traceparent(
                "00-zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz-b7ad6b7169203331-01"
            ));
        }

        #[test]
        fn test_extract_remote_span_context_valid() {
            init_propagator();
            let sc = extract_remote_span_context(
                Some("00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01"),
                None,
            );
            assert!(sc.is_some());
            let sc = sc.unwrap();
            assert_eq!(
                sc.trace_id(),
                TraceId::from_hex("0af7651916cd43dd8448eb211c80319c").unwrap()
            );
            assert!(sc.is_sampled());
        }

        #[test]
        fn test_extract_remote_span_context_invalid() {
            assert!(extract_remote_span_context(None, None).is_none());
            assert!(extract_remote_span_context(
                Some("00-zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz-b7ad6b7169203331-01"),
                None,
            )
            .is_none());
            assert!(extract_remote_span_context(
                Some("00-00000000000000000000000000000000-0000000000000000-01"),
                None,
            )
            .is_none());
        }

        #[test]
        fn test_extract_remote_span_context_with_tracestate() {
            let sc = extract_remote_span_context(
                Some("00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01"),
                Some("vendor1=value1"),
            );
            assert!(sc.is_some());
        }

        #[test]
        fn test_extract_remote_span_context_ambient_isolation_malformed_55char() {
            // Activate an unrelated ambient span, then supply a malformed
            // exactly-55-char traceparent with non-hex characters.
            // The propagator must NOT fall back to the ambient context.
            init_tracer();

            let parent_span = info_span!("ambient_span");
            let _guard = parent_span.entered();

            // 55 chars, valid format, but non-hex chars in trace-id
            let malformed = "00-zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz-b7ad6b7169203331-01";
            assert_eq!(malformed.len(), 55);

            let result = extract_remote_span_context(Some(malformed), None);
            assert!(
                result.is_none(),
                "must not return ambient context when traceparent has non-hex chars"
            );
        }

        #[test]
        fn test_extract_remote_span_context_ambient_isolation_valid() {
            // Activate an unrelated ambient span, then supply a valid but
            // different traceparent. The extracted context must match the
            // carrier, not the ambient.
            init_tracer();

            let parent_span = info_span!("ambient_span");
            let _guard = parent_span.entered();

            let carrier_tp = "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01";
            let result = extract_remote_span_context(Some(carrier_tp), None);
            assert!(
                result.is_some(),
                "valid traceparent must be extracted even with active ambient span"
            );
            let sc = result.unwrap();
            assert_eq!(
                sc.trace_id(),
                TraceId::from_hex("0af7651916cd43dd8448eb211c80319c").unwrap(),
                "extracted trace_id must come from carrier, not ambient"
            );
        }

        #[test]
        fn test_collect_span_contexts_deduplicates() {
            init_propagator();
            let tp = "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01";
            let items = vec![
                Some((Some(tp.to_string()), None)),
                Some((Some(tp.to_string()), None)),
            ];
            let result = collect_span_contexts(&items);
            assert_eq!(
                result.len(),
                1,
                "duplicate (trace_id, span_id) should be deduplicated"
            );
        }

        #[test]
        fn test_collect_span_contexts_multiple_traces() {
            init_propagator();
            let tp1 = "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01";
            let tp2 = "00-1bf7651916cd43dd8448eb211c80319c-b8ad6b7169203331-01";
            let items = vec![
                Some((Some(tp1.to_string()), None)),
                Some((Some(tp2.to_string()), None)),
            ];
            assert_eq!(collect_span_contexts(&items).len(), 2);
        }

        #[test]
        fn test_collect_span_contexts_same_trace_different_spans_retained() {
            init_propagator();
            let tp1 = "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01";
            let tp2 = "00-0af7651916cd43dd8448eb211c80319c-aaaaaaaaaaaaaaaa-01";
            let items = vec![
                Some((Some(tp1.to_string()), None)),
                Some((Some(tp2.to_string()), None)),
            ];
            let result = collect_span_contexts(&items);
            assert_eq!(
                result.len(),
                2,
                "two producer spans with same trace_id but different span_id must both be retained"
            );
        }

        #[test]
        fn test_collect_span_contexts_skips_invalid() {
            let items = vec![
                None,
                Some((None, None)),
                Some((Some("invalid".to_string()), None)),
            ];
            assert!(collect_span_contexts(&items).is_empty());
        }

        /// Set up a global tracer provider and tracing subscriber for tests
        /// that need to produce trace context.
        fn init_tracer() {
            init_propagator();
            static TRACER_INIT: std::sync::Once = std::sync::Once::new();
            TRACER_INIT.call_once(|| {
                let provider = opentelemetry_sdk::trace::SdkTracerProvider::builder().build();
                global::set_tracer_provider(provider.clone());
                let tracer = provider.tracer("test");
                let telemetry_layer = tracing_opentelemetry::layer().with_tracer(tracer);
                let _ = tracing_subscriber::registry()
                    .with(telemetry_layer)
                    .try_init();
            });
        }

        #[test]
        fn test_build_producer_span_and_inject() {
            init_tracer();
            let parent_span = info_span!("test");
            let _guard = parent_span.entered();
            let span = build_producer_span("test_queue");
            let carrier = {
                let _g = span.entered();
                inject_active_context()
            };
            assert!(carrier.traceparent.is_some());
            let tp = carrier.traceparent.unwrap();
            assert_eq!(tp.len(), 55);
            assert!(is_valid_traceparent(&tp));
        }

        #[test]
        fn test_trace_carrier_round_trip() {
            init_tracer();
            let parent_span = info_span!("test");
            let _guard = parent_span.entered();
            let span = build_producer_span("round_trip");
            let carrier = {
                let _g = span.entered();
                inject_active_context()
            };
            let tp = carrier.traceparent.clone().unwrap();
            let extracted = extract_remote_span_context(Some(&tp), carrier.tracestate.as_deref());
            assert!(extracted.is_some());
        }
    }
}
