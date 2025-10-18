import logging
import os
from opentelemetry import trace
from opentelemetry.exporter.otlp.proto.http.trace_exporter import OTLPSpanExporter
from opentelemetry.instrumentation.fastapi import FastAPIInstrumentor
from opentelemetry.instrumentation.httpx import HTTPXClientInstrumentor
from opentelemetry.sdk.resources import Resource, SERVICE_NAME, SERVICE_VERSION, DEPLOYMENT_ENVIRONMENT
from opentelemetry.sdk.trace import TracerProvider
from opentelemetry.sdk.trace.export import BatchSpanProcessor
from ulid import ULID

logger = logging.getLogger(__name__)


def init_telemetry(app, service_name: str = "omni-ai"):
    """
    Initialize OpenTelemetry instrumentation for the FastAPI application.
    """
    otlp_endpoint = os.getenv("OTEL_EXPORTER_OTLP_ENDPOINT")
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

    # Create tracer provider
    provider = TracerProvider(resource=resource)

    # Add OTLP exporter if endpoint is configured
    if otlp_endpoint:
        logger.info(f"Initializing OpenTelemetry with OTLP endpoint: {otlp_endpoint}")
        otlp_exporter = OTLPSpanExporter(endpoint=f"{otlp_endpoint}/v1/traces")
        processor = BatchSpanProcessor(otlp_exporter)
        provider.add_span_processor(processor)
    else:
        logger.info("No OTLP endpoint configured, telemetry will be collected locally only")

    # Set the global tracer provider
    trace.set_tracer_provider(provider)

    # Instrument FastAPI
    FastAPIInstrumentor.instrument_app(app)

    # Instrument HTTPX (for outbound HTTP requests)
    HTTPXClientInstrumentor().instrument()

    logger.info(
        f"Telemetry initialized for {service_name} "
        f"(deployment_id={deployment_id}, environment={environment})"
    )


def get_tracer(name: str = "omni-ai"):
    """
    Get a tracer instance for manual instrumentation.
    """
    return trace.get_tracer(name)
