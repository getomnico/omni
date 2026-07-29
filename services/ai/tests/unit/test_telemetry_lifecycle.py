"""Focused lifecycle tests for OTel LoggingHandler init/shutdown.

Tests use monkeypatching of production instruments/mocked exporters to avoid
contacting a real collector.  Covers:

- No-span formatter safety (TraceContextFilter)
- Active-span stdout trace_id/span_id
- Native exported context on log records
- Repeated init leaves one handler / one exported record
- Shutdown removes the handler
"""

import logging
from unittest.mock import MagicMock, patch, AsyncMock

import pytest


@pytest.fixture(autouse=True)
def _reset_globals():
    """Reset telemetry module globals before each test."""
    import telemetry as tel
    tel._tracer_provider = None
    tel._meter_provider = None
    tel._logger_provider = None
    tel._otel_log_handler = None
    # Remove any OTelLoggingHandlers left by previous tests
    root = logging.getLogger()
    for h in list(root.handlers):
        if "OTelLoggingHandler" in type(h).__name__:
            root.removeHandler(h)
    yield
    # Cleanup after test
    tel.shutdown_telemetry()
    for h in list(root.handlers):
        if "OTelLoggingHandler" in type(h).__name__:
            root.removeHandler(h)


@pytest.fixture
def mock_otlp_endpoint(monkeypatch):
    """Set OTEL_EXPORTER_OTLP_ENDPOINT so the log exporter path is
    exercised, but patch the exporters in the telemetry module to
    avoid real connections."""
    monkeypatch.setenv("OTEL_EXPORTER_OTLP_ENDPOINT", "http://127.0.0.1:1")
    # Patch at the telemetry module level (where they are imported)
    import telemetry as tel
    monkeypatch.setattr(tel, "OTLPLogExporter", MagicMock)
    monkeypatch.setattr(tel, "OTLPSpanExporter", MagicMock)
    monkeypatch.setattr(tel, "OTLPMetricExporter", MagicMock)


class TestNoSpanFormatterSafety:
    """TraceContextFilter must not KeyError when there is no active span."""

    def test_trace_context_filter_no_span(self):
        """Formatter safety: TraceContextFilter outside a span produces
        zero-padded trace_id/span_id and does not raise."""
        from telemetry import TraceContextFilter

        filt = TraceContextFilter()
        record = logging.LogRecord(
            name="test", level=logging.INFO, pathname="", lineno=0,
            msg="no-span test", args=(), exc_info=None,
        )
        result = filt.filter(record)
        assert result is True
        assert record.trace_id == "00000000000000000000000000000000"
        assert record.span_id == "0000000000000000"


class TestActiveSpanTraceContext:
    """Log records emitted within an active span carry native trace/span IDs."""

    def test_stdout_handler_gets_trace_ids(self, mock_otlp_endpoint, monkeypatch):
        """An active span's trace_id/span_id appear on stdout log records."""
        from opentelemetry import trace
        from opentelemetry.sdk.trace import TracerProvider
        from opentelemetry.sdk.trace.export import SimpleSpanProcessor
        import telemetry as tel

        # Use SimpleSpanProcessor without in-memory exporter since
        # InMemorySpanExporter may not be available in all versions.
        # We verify trace_id/span_id via a CaptureHandler instead.
        tracer_provider = TracerProvider()
        trace.set_tracer_provider(tracer_provider)

        # Create a mock app and init telemetry
        app = MagicMock()
        tel.init_telemetry(app)

        tracer = trace.get_tracer("test-tracer")

        # Capture a log record within an active span
        captured_records = []

        class CaptureHandler(logging.Handler):
            def emit(self, record):
                captured_records.append(record)

        cap_handler = CaptureHandler()
        # Add TraceContextFilter from telemetry module
        cap_handler.addFilter(tel.TraceContextFilter())
        root = logging.getLogger()
        root.addHandler(cap_handler)
        root.setLevel(logging.INFO)

        with tracer.start_as_current_span("test-span") as span:
            span_context = span.get_span_context()
            logging.getLogger("test").info("message within span")

        root.removeHandler(cap_handler)

        # Verify captured record has real trace IDs
        assert len(captured_records) >= 1
        for rec in captured_records:
            assert rec.trace_id != "00000000000000000000000000000000"

        tel.shutdown_telemetry()

    def test_native_exported_context(self):
        """Log records exported via OTel logger provider carry the
        active span's trace context natively.

        Uses real InMemoryLogRecordExporter to verify trace_id/span_id
        correlation and resource service.name propagation without an OTLP
        endpoint.
        """
        from opentelemetry import trace
        from opentelemetry.sdk.trace import TracerProvider
        from opentelemetry.sdk._logs import (
            LoggerProvider as LogLoggerProvider,
            LoggingHandler,
        )
        from opentelemetry.sdk._logs.export import (
            SimpleLogRecordProcessor,
            InMemoryLogRecordExporter,
        )
        from opentelemetry.sdk.resources import Resource, SERVICE_NAME

        # Set up tracer provider
        tracer_provider = TracerProvider()
        trace.set_tracer_provider(tracer_provider)

        # Build a logger provider with in-memory exporter for assertions
        memory_exporter = InMemoryLogRecordExporter()
        logger_provider = LogLoggerProvider(
            resource=Resource.create({
                SERVICE_NAME: "omni-ai",
            })
        )
        logger_provider.add_log_record_processor(
            SimpleLogRecordProcessor(memory_exporter)
        )

        # Create LoggingHandler that bridges stdlib logging to our logger provider.
        # Use DEBUG level so INFO and above are captured.
        otel_handler = LoggingHandler(
            level=logging.DEBUG,
            logger_provider=logger_provider,
        )
        # Set root logger to DEBUG so records reach the handler
        root = logging.getLogger()
        previous_level = root.level
        root.setLevel(logging.DEBUG)
        root.addHandler(otel_handler)

        try:
            tracer = trace.get_tracer("test-tracer")
            test_logger = logging.getLogger("oteltest-native")

            with tracer.start_as_current_span("export-span") as span:
                span_ctx = span.get_span_context()
                test_logger.info("exported log within span")

            # Force flush and get exported log records
            logger_provider.force_flush()
            exported = memory_exporter.get_finished_logs()

            # Assert at least one log record was exported
            assert len(exported) >= 1, (
                f"Expected at least 1 log record, got {len(exported)}"
            )

            # Find the record with our expected body (ReadableLogRecord.log_record.body)
            matching = [
                r for r in exported
                if r.log_record.body == "exported log within span"
            ]
            assert len(matching) >= 1, (
                f"No log record with expected body; bodies: {[r.log_record.body for r in exported]}"
            )

            log_record = matching[0]
            lr = log_record.log_record

            # Assert trace_id and span_id match the active span
            assert lr.trace_id == span_ctx.trace_id, (
                f"trace_id {lr.trace_id!r} != expected {span_ctx.trace_id!r}"
            )
            assert lr.span_id == span_ctx.span_id, (
                f"span_id {lr.span_id!r} != expected {span_ctx.span_id!r}"
            )

            # Assert the resource carries service.name
            resource = log_record.resource
            assert resource is not None
            svc_name = resource.attributes.get("service.name")
            assert svc_name is not None, "resource missing service.name"
            assert svc_name == "omni-ai", (
                f"service.name={svc_name!r}, expected 'omni-ai'"
            )
        finally:
            root.setLevel(previous_level)
            root.removeHandler(otel_handler)
            logger_provider.shutdown()
            tracer_provider.shutdown()
    """Repeated init leaves exactly one handler and one record per emit.
    Also verifies that exporting via the OTel LoggingHandler works correctly
    within an active span context.
    """

    def test_repeated_init_leaves_one_handler(self, mock_otlp_endpoint):
        """Two inits should result in exactly one OTelLoggingHandler
        attached to the root logger."""
        import telemetry as tel

        app = MagicMock()
        tel.init_telemetry(app)
        tel.init_telemetry(app)

        root = logging.getLogger()
        otel_handlers = [
            h for h in root.handlers
            if "LoggingHandler" in type(h).__name__
        ]
        assert len(otel_handlers) == 1, (
            f"Expected 1 OTelLoggingHandler after repeated init, "
            f"got {len(otel_handlers)}"
        )

    def test_repeated_init_produces_one_record(self, mock_otlp_endpoint):
        """One log emit after two inits should produce exactly one
        record through the OTel LoggingHandler."""
        import telemetry as tel

        app = MagicMock()
        tel.init_telemetry(app)
        tel.init_telemetry(app)

        # Verify only one OTelLoggingHandler exists
        root = logging.getLogger()
        otel_handlers = [
            h for h in root.handlers
            if "LoggingHandler" in type(h).__name__
        ]
        assert len(otel_handlers) == 1

        # Verify that logging through the root logger reaches the handler
        # by counting handler calls
        handler = otel_handlers[0]
        original_handle = handler.handle
        handle_count = 0

        def counting_handle(record):
            nonlocal handle_count
            handle_count += 1
            return original_handle(record)

        handler.handle = counting_handle

        test_logger = logging.getLogger("test-single-emit")
        test_logger.info("single emit")

        # handle should be called exactly once
        assert handle_count == 1, (
            f"Expected 1 handle call, got {handle_count}"
        )


class TestShutdown:
    """Shutdown removes the handler and leaves zero OTelLoggingHandlers."""

    def test_shutdown_removes_handler(self, mock_otlp_endpoint):
        """After shutdown, no OTelLoggingHandler remains on the root logger."""
        import telemetry as tel

        app = MagicMock()
        tel.init_telemetry(app)

        tel.shutdown_telemetry()

        root = logging.getLogger()
        otel_handlers = [
            h for h in root.handlers
            if "LoggingHandler" in type(h).__name__
        ]
        assert len(otel_handlers) == 0, (
            f"Expected 0 OTelLoggingHandler after shutdown, "
            f"got {len(otel_handlers)}"
        )

    def test_shutdown_also_removes_reinit_handler(self, mock_otlp_endpoint):
        """After reinit and shutdown, zero handlers remain."""
        import telemetry as tel

        app = MagicMock()
        tel.init_telemetry(app)
        tel.init_telemetry(app)
        tel.shutdown_telemetry()

        root = logging.getLogger()
        otel_handlers = [
            h for h in root.handlers
            if "LoggingHandler" in type(h).__name__
        ]
        assert len(otel_handlers) == 0
