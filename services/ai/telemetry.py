"""OpenTelemetry initialization for omni-ai (traces + metrics + logs)."""

import logging
import os
from typing import Optional

from opentelemetry import trace, metrics
from opentelemetry.exporter.otlp.proto.http.trace_exporter import OTLPSpanExporter
from opentelemetry.exporter.otlp.proto.http.metric_exporter import OTLPMetricExporter
from opentelemetry.exporter.otlp.proto.http._log_exporter import OTLPLogExporter
from opentelemetry.instrumentation.fastapi import FastAPIInstrumentor
from opentelemetry.instrumentation.httpx import HTTPXClientInstrumentor
from opentelemetry.sdk.resources import (
    Resource,
    SERVICE_NAME,
    SERVICE_VERSION,
    DEPLOYMENT_ENVIRONMENT,
)
from opentelemetry.sdk.trace import TracerProvider
from opentelemetry.sdk.trace.export import BatchSpanProcessor
from opentelemetry.sdk.metrics import MeterProvider
from opentelemetry.sdk.metrics.export import PeriodicExportingMetricReader
from opentelemetry.sdk._logs import LoggerProvider as LogLoggerProvider
from opentelemetry.sdk._logs.export import BatchLogRecordProcessor
from opentelemetry.sdk._logs import LoggingHandler as OTelLoggingHandler
from ulid import ULID

logger = logging.getLogger(__name__)

# Hold references so we can shut them down gracefully.
_tracer_provider: TracerProvider | None = None
_meter_provider: MeterProvider | None = None
_logger_provider: LogLoggerProvider | None = None
_otel_log_handler: OTelLoggingHandler | None = None


# ---------------------------------------------------------------------------
# Production helpers
# ---------------------------------------------------------------------------


def normalize_otlp_endpoint(endpoint_raw: str | None) -> str | None:
    """Normalise an OTLP endpoint: strip trailing slash, treat empty as None."""
    if endpoint_raw:
        return endpoint_raw.rstrip("/")
    return None


def parse_metric_export_interval() -> int:
    """Parse OTEL_METRIC_EXPORT_INTERVAL as a finite positive integer
    (milliseconds). Falls back to 60000 (60 s) when unset or invalid."""
    raw = os.environ.get("OTEL_METRIC_EXPORT_INTERVAL")
    if raw is not None:
        try:
            val = int(raw)
            if val > 0:
                return val
        except (ValueError, TypeError):
            pass
    return 60_000


def _build_otlp_logs_url(endpoint: str) -> str:
    """Build the OTLP HTTP logs export URL from a base endpoint."""
    return f"{endpoint}/v1/logs"


class TraceContextFilter(logging.Filter):
    """Logging filter that adds bounded ``trace_id`` and ``span_id`` fields
    to every ``LogRecord``.

    Inside an active OTel span the real trace/span IDs are recorded.
    Outside any span the fields are ``"00000000000000000000000000000000"``
    (trace_id) / ``"0000000000000000"`` (span_id).  This prevents formatter
    KeyErrors before telemetry initialisation.
    """

    def filter(self, record: logging.LogRecord) -> bool:
        span = trace.get_current_span()
        span_context = span.get_span_context()

        if span_context and span_context.is_valid:
            record.trace_id = span_context.trace_id
            record.span_id = span_context.span_id
        else:
            record.trace_id = "00000000000000000000000000000000"
            record.span_id = "0000000000000000"
        return True


def init_telemetry(app, service_name: str = "omni-ai"):
    """
    Initialize OpenTelemetry instrumentation for the FastAPI application.

    Idempotent: if called a second time, the prior OTel LoggingHandler is
    removed and the prior logger provider is shut down before creating a new
    one.  This prevents handler accumulation during repeated test init.
    """
    global _tracer_provider, _meter_provider, _logger_provider, _otel_log_handler

    # --- Idempotent cleanup: remove prior OTel LoggingHandler ---
    root_logger = logging.getLogger()
    if _otel_log_handler is not None:
        root_logger.removeHandler(_otel_log_handler)
        _otel_log_handler = None
    # Remove any remaining OTel LoggingHandlers that might have been
    # installed by a prior init without going through our global.
    for h in list(root_logger.handlers):
        if isinstance(h, OTelLoggingHandler):
            root_logger.removeHandler(h)

    # Shut down prior logger provider (if any) so we don't leak one
    # per init call.  Do NOT replace global trace/meter providers here
    # — those are set once and shared.
    if _logger_provider is not None:
        try:
            _logger_provider.shutdown()
        except Exception:
            pass
        _logger_provider = None

    otlp_endpoint = normalize_otlp_endpoint(os.getenv("OTEL_EXPORTER_OTLP_ENDPOINT"))
    deployment_id = os.getenv("OTEL_DEPLOYMENT_ID", str(ULID()))
    environment = os.getenv("OTEL_DEPLOYMENT_ENVIRONMENT", "development")
    service_version = os.getenv("SERVICE_VERSION", "0.1.0")

    # Create resource with service information
    resource = Resource(
        attributes={
            SERVICE_NAME: service_name,
            SERVICE_VERSION: service_version,
            DEPLOYMENT_ENVIRONMENT: environment,
            "deployment.id": deployment_id,
        }
    )

    # ------------------------------------------------------------------
    # Tracer provider
    # ------------------------------------------------------------------
    tracer_provider = TracerProvider(resource=resource)

    if otlp_endpoint:
        logger.info(f"Initializing OpenTelemetry with OTLP endpoint: {otlp_endpoint}")
        otlp_exporter = OTLPSpanExporter(endpoint=f"{otlp_endpoint}/v1/traces")
        processor = BatchSpanProcessor(otlp_exporter)
        tracer_provider.add_span_processor(processor)
    else:
        logger.info(
            "No OTLP endpoint configured, traces will be collected locally only"
        )

    trace.set_tracer_provider(tracer_provider)
    _tracer_provider = tracer_provider

    # ------------------------------------------------------------------
    # Meter provider (metrics)
    # ------------------------------------------------------------------
    if otlp_endpoint:
        metric_exporter = OTLPMetricExporter(endpoint=f"{otlp_endpoint}/v1/metrics")
        metric_reader = PeriodicExportingMetricReader(
            metric_exporter,
            export_interval_millis=parse_metric_export_interval(),
            export_timeout_millis=30_000,
        )
    else:
        metric_reader = None
        logger.info(
            "No OTLP endpoint configured, metrics will be collected locally only"
        )

    if metric_reader:
        meter_provider = MeterProvider(
            resource=resource,
            metric_readers=[metric_reader],
        )
    else:
        meter_provider = MeterProvider(resource=resource)

    metrics.set_meter_provider(meter_provider)
    _meter_provider = meter_provider

    # ------------------------------------------------------------------
    # Logger provider (logs)
    # ------------------------------------------------------------------
    if otlp_endpoint:
        log_exporter = OTLPLogExporter(
            endpoint=_build_otlp_logs_url(otlp_endpoint)
        )
        logger_provider = LogLoggerProvider(resource=resource)
        logger_provider.add_log_record_processor(
            BatchLogRecordProcessor(log_exporter)
        )
    else:
        logger_provider = LogLoggerProvider(resource=resource)

    _logger_provider = logger_provider

    # Install a LoggingHandler that bridges stdlib logging to OTel logs,
    # using the same resource and logger provider.  A TraceContextFilter
    # ensures that every LogRecord carries bounded trace_id / span_id
    # fields.
    otel_handler = OTelLoggingHandler(
        level=logging.NOTSET,
        logger_provider=logger_provider,
    )
    otel_handler.addFilter(TraceContextFilter())
    _otel_log_handler = otel_handler

    # Attach the OTel handler to the root logger.  Avoid duplicate handlers
    # when init is called repeatedly (tests).
    root_logger = logging.getLogger()
    if otel_handler not in root_logger.handlers:
        root_logger.addHandler(otel_handler)

    # Attach TraceContextFilter to all existing root handlers so stdout
    # records always have trace_id/span_id fields.
    for h in root_logger.handlers:
        if not any(isinstance(f, TraceContextFilter) for f in h.filters):
            h.addFilter(TraceContextFilter())

    # ------------------------------------------------------------------
    # Instrument FastAPI
    # ------------------------------------------------------------------
    FastAPIInstrumentor.instrument_app(app)

    # Instrument HTTPX (for outbound HTTP requests)
    HTTPXClientInstrumentor().instrument()

    logger.info(
        f"Telemetry initialized for {service_name} "
        f"(deployment_id={deployment_id}, environment={environment})"
    )


def shutdown_telemetry():
    """Shut down and flush the tracer, meter, and logger providers.

    Removes every OTel LoggingHandler created by this module so the root
    logger is left with zero handlers from this init.
    """
    global _tracer_provider, _meter_provider, _logger_provider, _otel_log_handler

    # Remove every OTel LoggingHandler from the root logger so we leave
    # zero handlers behind, regardless of how many inits were called.
    root_logger = logging.getLogger()
    if _otel_log_handler is not None:
        root_logger.removeHandler(_otel_log_handler)
        _otel_log_handler = None
    for h in list(root_logger.handlers):
        if isinstance(h, OTelLoggingHandler):
            root_logger.removeHandler(h)

    # Shut down meter provider first (flush pending metric exports)
    if _meter_provider is not None:
        logger.info("Shutting down OpenTelemetry meter provider")
        try:
            _meter_provider.shutdown()
        except Exception:
            logger.exception("Error shutting down meter provider")
        _meter_provider = None

    if _tracer_provider is not None:
        logger.info("Shutting down OpenTelemetry tracer provider")
        try:
            _tracer_provider.shutdown()
        except Exception:
            logger.exception("Error shutting down tracer provider")
        _tracer_provider = None

    # Shut down logger provider last so final correlated log records are
    # exported before the provider is torn down.
    if _logger_provider is not None:
        logger.info("Shutting down OpenTelemetry logger provider")
        try:
            _logger_provider.shutdown()
        except Exception:
            logger.exception("Error shutting down logger provider")
        _logger_provider = None


def get_tracer(name: str = "omni-ai"):
    """
    Get a tracer instance for manual instrumentation.
    """
    return trace.get_tracer(name)


def get_meter(name: str = "omni-ai"):
    """
    Get a meter instance for manual instrumentation.
    """
    return metrics.get_meter(name)
