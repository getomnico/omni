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
