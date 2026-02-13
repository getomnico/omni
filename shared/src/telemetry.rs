use anyhow::Result;
use opentelemetry::{global, trace::TracerProvider as _, KeyValue};
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::{
    trace::{RandomIdGenerator, Sampler, TracerProvider},
    Resource,
};
use opentelemetry_semantic_conventions::{
    resource::{SERVICE_NAME, SERVICE_VERSION},
    SCHEMA_URL,
};
use std::time::Duration;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, Layer};

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
            otlp_endpoint: std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").ok(),
            deployment_id: std::env::var("OTEL_DEPLOYMENT_ID")
                .unwrap_or_else(|_| ulid::Ulid::new().to_string()),
            environment: std::env::var("OTEL_DEPLOYMENT_ENVIRONMENT")
                .unwrap_or_else(|_| "development".to_string()),
            service_version: std::env::var("SERVICE_VERSION")
                .unwrap_or_else(|_| env!("CARGO_PKG_VERSION").to_string()),
        }
    }
}

pub fn init_telemetry(config: TelemetryConfig) -> Result<()> {
    let resource = Resource::from_schema_url(
        [
            KeyValue::new(SERVICE_NAME, config.service_name.clone()),
            KeyValue::new(SERVICE_VERSION, config.service_version.clone()),
            KeyValue::new("deployment.environment", config.environment.clone()),
            KeyValue::new("deployment.id", config.deployment_id.clone()),
        ],
        SCHEMA_URL,
    );

    let otlp_endpoint_for_log = config.otlp_endpoint.clone();

    let tracer_provider = if let Some(endpoint) = config.otlp_endpoint {
        let exporter = opentelemetry_otlp::SpanExporter::builder()
            .with_http()
            .with_endpoint(&endpoint)
            .with_timeout(Duration::from_secs(10))
            .build()?;

        TracerProvider::builder()
            .with_batch_exporter(exporter, opentelemetry_sdk::runtime::Tokio)
            .with_resource(resource)
            .with_sampler(Sampler::AlwaysOn)
            .with_id_generator(RandomIdGenerator::default())
            .build()
    } else {
        TracerProvider::builder()
            .with_resource(resource)
            .with_sampler(Sampler::AlwaysOn)
            .with_id_generator(RandomIdGenerator::default())
            .build()
    };

    global::set_tracer_provider(tracer_provider.clone());

    let tracer = tracer_provider.tracer(config.service_name.clone());
    let telemetry_layer = tracing_opentelemetry::layer().with_tracer(tracer);

    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info"))
        .add_directive("sqlx=warn".parse()?)
        .add_directive("hyper=info".parse()?)
        .add_directive("tower_http=info".parse()?);

    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_target(true)
        .with_level(true)
        .with_thread_ids(false)
        .with_file(false)
        .with_line_number(false)
        .json()
        .with_filter(env_filter);

    tracing_subscriber::registry()
        .with(telemetry_layer)
        .with(fmt_layer)
        .init();

    tracing::info!(
        service_name = %config.service_name,
        deployment_id = %config.deployment_id,
        environment = %config.environment,
        otlp_endpoint = ?otlp_endpoint_for_log,
        "Telemetry initialized"
    );

    Ok(())
}

pub async fn shutdown_telemetry() {
    tracing::info!("Shutting down telemetry");
    global::shutdown_tracer_provider();
}

pub mod middleware {
    use axum::{extract::Request, http::HeaderMap, middleware::Next, response::Response};
    use opentelemetry::{
        global,
        trace::{SpanKind, TraceContextExt, Tracer},
        Context,
    };
    use opentelemetry_http::{HeaderExtractor, HeaderInjector};
    use tracing::{Instrument, Span};
    use tracing_opentelemetry::OpenTelemetrySpanExt;

    pub async fn trace_layer(mut request: Request, next: Next) -> Response {
        let headers = request.headers();
        let parent_context = global::get_text_map_propagator(|propagator| {
            propagator.extract(&HeaderExtractor(headers))
        });

        let tracer = global::tracer("http-server");
        let span_builder = tracer
            .span_builder(format!("{} {}", request.method(), request.uri().path()))
            .with_kind(SpanKind::Server);

        let otel_span = tracer.build_with_context(span_builder, &parent_context);
        let context = Context::current_with_span(otel_span);

        let tracing_span = tracing::info_span!(
            "http_request",
            method = %request.method(),
            uri = %request.uri(),
            version = ?request.version(),
        );

        tracing_span.set_parent(context.clone());

        let request_id = {
            let span = context.span();
            span.span_context().trace_id().to_string()
        };

        request.extensions_mut().insert(request_id.clone());

        let response = next.run(request).instrument(tracing_span.clone()).await;

        tracing_span.in_scope(|| {
            tracing::info!(
                status = response.status().as_u16(),
                request_id = %request_id,
                "Request completed"
            );
        });

        response
    }

    pub fn inject_trace_context(headers: &mut HeaderMap, span: &Span) {
        let context = span.context();
        let mut injector = HeaderInjector(headers);
        global::get_text_map_propagator(|propagator| {
            propagator.inject_context(&context, &mut injector);
        });
    }

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
    use axum::http::HeaderMap;
    use opentelemetry::global;
    use opentelemetry_http::HeaderInjector;
    use reqwest::RequestBuilder;
    use tracing::Span;
    use tracing_opentelemetry::OpenTelemetrySpanExt;

    pub fn inject_trace_headers(mut builder: RequestBuilder) -> RequestBuilder {
        let span = Span::current();
        let context = span.context();

        let mut headers = HeaderMap::new();
        let mut injector = HeaderInjector(&mut headers);

        global::get_text_map_propagator(|propagator| {
            propagator.inject_context(&context, &mut injector);
        });

        for (key, value) in headers.iter() {
            builder = builder.header(key.clone(), value.clone());
        }

        builder
    }

    pub trait RequestBuilderExt {
        fn with_trace_context(self) -> Self;
    }

    impl RequestBuilderExt for RequestBuilder {
        fn with_trace_context(self) -> Self {
            inject_trace_headers(self)
        }
    }
}
