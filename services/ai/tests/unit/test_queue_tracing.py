"""Queue tracing tests for the embedding batch processor.

Uses InMemorySpanExporter to verify that CONSUMER spans carry native links
to their PRODUCER contexts, that failure scenarios set ERROR status, and
that invalid/missing contexts are ignored.
"""

from datetime import datetime, timezone
from unittest.mock import AsyncMock, MagicMock

import pytest
from opentelemetry import trace
from opentelemetry.context import Context
from opentelemetry.propagate import set_global_textmap
from opentelemetry.sdk.trace import TracerProvider
from opentelemetry.sdk.trace.export import SimpleSpanProcessor
from opentelemetry.sdk.trace.export.in_memory_span_exporter import InMemorySpanExporter
from opentelemetry.trace import SpanKind, StatusCode
from opentelemetry.trace.propagation.tracecontext import TraceContextTextMapPropagator

from db.embedding_queue import EmbeddingQueueItem
from embeddings.batch_processor import (
    _build_carrier,
    _extract_span_context,
    _build_consumer_links,
    EmbeddingBatchProcessor,
)


# ---------------------------------------------------------------------------
# Fixtures
# ---------------------------------------------------------------------------


# ---------------------------------------------------------------------------
# Module-level shared tracer provider and exporter.
# Initialised at import time so `trace.set_tracer_provider` is called BEFORE
# any other test file can lock it (the Python OTel API only allows one set).
# ---------------------------------------------------------------------------
set_global_textmap(TraceContextTextMapPropagator())
_SHARED_EXPORTER = InMemorySpanExporter()
_SHARED_PROVIDER = TracerProvider()
_SHARED_PROVIDER.add_span_processor(SimpleSpanProcessor(_SHARED_EXPORTER))
trace.set_tracer_provider(_SHARED_PROVIDER)


@pytest.fixture(autouse=True)
def _reset_tracing():
    """Reset batcher instrument globals and exporter before each test."""
    _SHARED_EXPORTER.clear()
    # Reset the batcher's instrument globals so each test starts clean
    import embeddings.batch_processor as bp

    bp._EMBEDDING_METER = None
    bp._EMBEDDING_PENDING = None
    bp._EMBEDDING_PROCESSED = None
    bp._EMBEDDING_FAILED = None
    bp._EMBEDDING_BATCH_DURATION = None
    yield


@pytest.fixture
def span_exporter():
    """Return the shared InMemorySpanExporter."""
    return _SHARED_EXPORTER


def _make_processor(span_exporter) -> EmbeddingBatchProcessor:
    """Build a processor with all repos mocked."""
    docs_repo = AsyncMock()
    queue_repo = AsyncMock()
    embeddings_repo = AsyncMock()
    app_state = MagicMock()
    # Use MagicMock for the embedding provider (sync methods like get_model_name)
    # with specific AsyncMock for async methods like generate_embeddings.
    provider = MagicMock()
    provider.get_model_name.return_value = "test-model"
    provider.generate_embeddings = AsyncMock(
        return_value=[MagicMock(span=(0, 4), embedding=[0.1])]
    )
    app_state.embedding_provider = provider
    app_state.embedding_provider_type = "test"
    app_state.content_storage = AsyncMock()

    # Default find_embedded_content_donors to empty dict so that
    # _clone_same_content_embeddings always short-circuits and never
    # produces unawaited AsyncMock coroutine warnings.
    docs_repo.find_embedded_content_donors = AsyncMock(return_value={})

    processor = EmbeddingBatchProcessor(
        documents_repo=docs_repo,
        queue_repo=queue_repo,
        embeddings_repo=embeddings_repo,
        app_state=app_state,
    )
    processor._baseline_completed = 0
    processor._baseline_failed = 0
    return processor


def _make_item(
    document_id: str,
    traceparent: str | None = None,
    tracestate: str | None = None,
) -> EmbeddingQueueItem:
    return EmbeddingQueueItem(
        id=f"item-{document_id}",
        document_id=document_id,
        status="processing",
        error_message=None,
        retry_count=0,
        created_at=datetime.now(timezone.utc),
        traceparent=traceparent,
        tracestate=tracestate,
    )


def _find_span(spans, name: str, kind: SpanKind):
    """Find a finished span by name and kind."""
    for s in spans:
        if s.name == name and s.kind == kind:
            return s
    return None


def _span_context_from_traceparent(tp: str):
    """Extract a SpanContext from a traceparent for link assertions."""
    sc = _extract_span_context(tp, None)
    assert sc is not None, f"could not extract from {tp}"
    return sc


def _flush_exporter():
    """Force-flush the global tracer provider if available."""
    tracer_provider = trace.get_tracer_provider()
    if hasattr(tracer_provider, "force_flush"):
        tracer_provider.force_flush()


# ---------------------------------------------------------------------------
# Pure helpers
# ---------------------------------------------------------------------------


class TestBuildCarrier:
    def test_valid_traceparent(self):
        tp = "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01"
        carrier = _build_carrier(tp, None)
        assert carrier["traceparent"] == tp
        assert "tracestate" not in carrier

    def test_with_tracestate(self):
        tp = "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01"
        carrier = _build_carrier(tp, "vendor1=value1")
        assert carrier["traceparent"] == tp
        assert carrier["tracestate"] == "vendor1=value1"

    def test_invalid_format_returns_empty(self):
        assert _build_carrier("invalid", None) == {}
        assert _build_carrier("", None) == {}
        assert _build_carrier(None, None) == {}
        assert _build_carrier("00-too-short", None) == {}


class TestExtractSpanContext:
    def test_valid(self):
        tp = "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01"
        sc = _extract_span_context(tp, None)
        assert sc is not None
        assert sc.is_valid

    def test_invalid_returns_none(self):
        assert _extract_span_context(None, None) is None
        assert _extract_span_context("invalid", None) is None
        assert _extract_span_context("", None) is None

    def test_all_zero_returns_invalid(self):
        tp = "00-00000000000000000000000000000000-0000000000000000-01"
        sc = _extract_span_context(tp, None)
        assert sc is None or not sc.is_valid

    def test_ambient_span_not_leaked_when_malformed(self):
        """Active ambient span must NOT leak into extract with malformed traceparent."""
        tracer = trace.get_tracer("test")
        with tracer.start_as_current_span("ambient"):
            # Exactly 55 chars, valid format but non-hex trace id
            malformed = "00-zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz-b7ad6b7169203331-01"
            assert len(malformed) == 55
            sc = _extract_span_context(malformed, None)
            assert sc is None, "must not return ambient context for non-hex traceparent"

    def test_ambient_span_not_leaked_when_valid(self):
        """Active ambient span must NOT replace the carrier context on valid extract."""
        tracer = trace.get_tracer("test")
        with tracer.start_as_current_span("ambient"):
            tp = "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01"
            sc = _extract_span_context(tp, None)
            assert sc is not None
            assert sc.is_valid
            expected_tid = int("0af7651916cd43dd8448eb211c80319c", 16)
            assert sc.trace_id == expected_tid, \
                "trace_id must come from carrier, not ambient span"


class TestBuildConsumerLinks:
    def test_single_valid_item(self):
        tp = "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01"
        items = [_make_item("doc1", traceparent=tp)]
        links = _build_consumer_links(items)
        assert len(links) == 1
        expected_tid = int("0af7651916cd43dd8448eb211c80319c", 16)
        assert links[0].context.trace_id == expected_tid

    def test_deduplicates_same_context(self):
        tp = "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01"
        items = [
            _make_item("doc1", traceparent=tp),
            _make_item("doc2", traceparent=tp),
        ]
        links = _build_consumer_links(items)
        assert len(links) == 1

    def test_same_trace_different_spans_retained(self):
        tp1 = "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01"
        tp2 = "00-0af7651916cd43dd8448eb211c80319c-aaaaaaaaaaaaaaaa-01"
        items = [
            _make_item("doc1", traceparent=tp1),
            _make_item("doc2", traceparent=tp2),
        ]
        links = _build_consumer_links(items)
        assert len(links) == 2

    def test_same_trace_three_items_mixed(self):
        """Three items: two sharing one span, one unique, one invalid."""
        tp1 = "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01"
        tp2 = "00-0af7651916cd43dd8448eb211c80319c-aaaaaaaaaaaaaaaa-01"
        items = [
            _make_item("doc1", traceparent=tp1),
            _make_item("doc2", traceparent=tp2),
            _make_item("doc3", traceparent=tp1),  # duplicate of doc1
            _make_item("doc4", traceparent=None),  # invalid
        ]
        links = _build_consumer_links(items)
        assert len(links) == 2

    def test_invalid_skipped(self):
        items = [
            _make_item("doc1", traceparent=None),
            _make_item("doc2", traceparent="invalid"),
        ]
        links = _build_consumer_links(items)
        assert len(links) == 0


# ---------------------------------------------------------------------------
# Batch processor tracing integration
# ---------------------------------------------------------------------------


class TestProcessOnlineBatchTracing:
    """Trace behaviour of _process_online_batch via in-memory exporter."""

    # -- Helpers to set up common mocks --------------------------------

    @staticmethod
    def _setup_happy_path(processor, items):
        """Configure mocks for a successful processing path."""
        processor.queue_repo.get_pending_items.return_value = items
        # Create realistic Document mocks: content_id set, external_id=None to
        # avoid triggering cross-source dedup path.
        docs = {}
        for item in items:
            doc = MagicMock()
            doc.content_id = f"c-{item.document_id}"
            doc.external_id = None
            docs[item.document_id] = doc
        processor.documents_repo.get_by_ids.return_value = docs
        processor.content_storage.get_text = AsyncMock(return_value="some text content")
        processor.embeddings_repo.bulk_insert = AsyncMock()
        # Ensure _clone_same_content_embeddings returns items as-is
        processor.documents_repo.find_embedded_content_donors = AsyncMock(return_value={})

    @pytest.mark.asyncio
    async def test_consumer_span_is_new_root(self, span_exporter):
        """CONSUMER span has no parent (new root trace)."""
        processor = _make_processor(span_exporter)

        tp = "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01"
        self._setup_happy_path(processor, [_make_item("doc1", traceparent=tp)])

        result = await processor._process_online_batch()
        assert result is True

        _flush_exporter()
        spans = span_exporter.get_finished_spans()
        span_exporter.clear()

        consumer = _find_span(spans, "embedding_queue process", SpanKind.CONSUMER)
        assert consumer is not None, "CONSUMER span must be exported"

        # Verify parent is None (new root trace via explicit Context())
        assert consumer.parent is None, (
            "CONSUMER span must have no parent (new root)"
        )

        # Verify consumer trace ID differs from producer trace ID
        link = consumer.links[0]
        assert consumer.context.trace_id != link.context.trace_id, (
            "CONSUMER must have different trace than producer"
        )

    @pytest.mark.asyncio
    async def test_native_link_matches_producer(self, span_exporter):
        """CONSUMER span has a native link matching the producer traceparent."""
        processor = _make_processor(span_exporter)

        tp = "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01"
        self._setup_happy_path(processor, [_make_item("doc1", traceparent=tp)])

        await processor._process_online_batch()

        _flush_exporter()
        spans = span_exporter.get_finished_spans()
        span_exporter.clear()

        consumer = _find_span(spans, "embedding_queue process", SpanKind.CONSUMER)
        assert consumer is not None
        assert len(consumer.links) >= 1, "must have at least one link"

        # Verify link matches the expected producer context
        expected_sc = _span_context_from_traceparent(tp)
        link = consumer.links[0]
        assert link.context.trace_id == expected_sc.trace_id, (
            "link trace_id must match producer trace_id"
        )
        assert link.context.span_id == expected_sc.span_id, (
            "link span_id must match producer span_id"
        )
        assert consumer.parent is None, (
            "CONSUMER must be new root"
        )

    @pytest.mark.asyncio
    async def test_same_trace_two_producers_two_links(self, span_exporter):
        """Two items sharing same trace but different span IDs create two links."""
        processor = _make_processor(span_exporter)

        tp1 = "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01"
        tp2 = "00-0af7651916cd43dd8448eb211c80319c-aaaaaaaaaaaaaaaa-01"
        item1 = _make_item("doc1", traceparent=tp1)
        item2 = _make_item("doc2", traceparent=tp2)
        self._setup_happy_path(processor, [item1, item2])

        await processor._process_online_batch()

        _flush_exporter()
        spans = span_exporter.get_finished_spans()
        span_exporter.clear()

        consumer = _find_span(spans, "embedding_queue process", SpanKind.CONSUMER)
        assert consumer is not None
        assert len(consumer.links) == 2, "must have 2 links for 2 distinct producers"

        # Both links should have the same trace_id
        link_tids = {l.context.trace_id for l in consumer.links}
        assert len(link_tids) == 1, "both links must share the same trace_id"

        # But different span_ids
        link_sids = {l.context.span_id for l in consumer.links}
        assert len(link_sids) == 2, "links must have different span_ids"

    @pytest.mark.asyncio
    async def test_invalid_traceparent_ignored(self, span_exporter):
        """Items with invalid/missing traceparent do not create links."""
        processor = _make_processor(span_exporter)

        item1 = _make_item("doc1", traceparent=None)
        item2 = _make_item("doc2", traceparent="invalid")
        self._setup_happy_path(processor, [item1, item2])

        await processor._process_online_batch()

        _flush_exporter()
        spans = span_exporter.get_finished_spans()
        span_exporter.clear()

        consumer = _find_span(spans, "embedding_queue process", SpanKind.CONSUMER)
        if consumer is not None:
            assert len(consumer.links) == 0, "no valid producer contexts"

    @pytest.mark.asyncio
    async def test_processing_runs_inside_consumer(self, span_exporter):
        """Processing operations occur within the CONSUMER span scope."""
        processor = _make_processor(span_exporter)

        tp = "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01"
        item = _make_item("doc1", traceparent=tp)
        processor.queue_repo.get_pending_items.return_value = [item]
        processor.documents_repo.get_by_ids.return_value = {
            "doc1": MagicMock(content_id="c1")
        }
        emb_provider = processor.embedding_provider
        emb_provider.get_model_name.return_value = "test-model"

        # Track which span is active during processing
        captured_span_context = None

        async def track_span(item, doc):
            nonlocal captured_span_context
            current_span = trace.get_current_span()
            sc = current_span.get_span_context()
            captured_span_context = sc

        processor._process_single_document = track_span

        await processor._process_online_batch()

        _flush_exporter()
        spans = span_exporter.get_finished_spans()
        span_exporter.clear()

        consumer = _find_span(spans, "embedding_queue process", SpanKind.CONSUMER)
        assert consumer is not None

        # Verify the active span during processing matches the consumer span
        assert captured_span_context is not None
        assert captured_span_context.trace_id == consumer.context.trace_id, (
            "trace_id must match consumer span"
        )

    @pytest.mark.asyncio
    async def test_item_failure_sets_error_status(self, span_exporter):
        """When an item fails, the CONSUMER span has ERROR status."""
        processor = _make_processor(span_exporter)

        tp = "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01"
        item = _make_item("doc1", traceparent=tp)
        processor.queue_repo.get_pending_items.return_value = [item]
        processor.documents_repo.get_by_ids.return_value = {
            "doc1": MagicMock(content_id="c1")
        }
        emb_provider = processor.embedding_provider
        emb_provider.get_model_name.return_value = "test-model"

        # Make processing fail
        async def fail_process(item, doc):
            raise RuntimeError("test failure")

        processor._process_single_document = fail_process
        processor.queue_repo.mark_failed = AsyncMock()

        await processor._process_online_batch()

        _flush_exporter()
        spans = span_exporter.get_finished_spans()
        span_exporter.clear()

        consumer = _find_span(spans, "embedding_queue process", SpanKind.CONSUMER)
        assert consumer is not None

        # Check status is ERROR (StatusCode.ERROR = 2)
        assert consumer.status is not None
        assert consumer.status.status_code == StatusCode.ERROR, (
            "span must have ERROR status when item fails"
        )

    @pytest.mark.asyncio
    async def test_missing_content_id_failure_real_path(self, span_exporter):
        """Real _process_single_document: missing content_id returns False and
        sets consumer span ERROR status."""
        processor = _make_processor(span_exporter)

        tp = "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01"
        item = _make_item("doc1", traceparent=tp)
        processor.queue_repo.get_pending_items.return_value = [item]
        # Document with no content_id
        doc = MagicMock(spec_set=["content_id", "external_id"])
        doc.content_id = None
        doc.external_id = None
        processor.documents_repo.get_by_ids.return_value = {"doc1": doc}
        processor.documents_repo.find_embedded_content_donors = AsyncMock(return_value={})

        # Use the REAL _process_single_document (no replacement)
        result = await processor._process_online_batch()
        assert result is True

        _flush_exporter()
        spans = span_exporter.get_finished_spans()
        span_exporter.clear()

        consumer = _find_span(spans, "embedding_queue process", SpanKind.CONSUMER)
        assert consumer is not None, "CONSUMER span must be exported"

        # The missing content_id path sets had_failure -> ERROR
        assert consumer.status is not None
        assert consumer.status.status_code == StatusCode.ERROR, (
            "span must have ERROR status when content_id is missing"
        )

    @pytest.mark.asyncio
    async def test_embedding_exception_failure_real_path(self, span_exporter):
        """Real _process_single_document: embedding provider exception returns
        False and sets consumer span ERROR status."""
        processor = _make_processor(span_exporter)

        tp = "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01"
        item = _make_item("doc1", traceparent=tp)
        processor.queue_repo.get_pending_items.return_value = [item]
        # Document with content_id so we reach the embedding call
        doc = MagicMock(spec_set=["content_id", "external_id"])
        doc.content_id = "c-doc1"
        doc.external_id = None
        processor.documents_repo.get_by_ids.return_value = {"doc1": doc}
        processor.documents_repo.find_embedded_content_donors = AsyncMock(return_value={})
        processor.content_storage.get_text = AsyncMock(
            return_value="some text content"
        )
        # Make embedding provider raise an exception
        provider = processor.embedding_provider
        provider.generate_embeddings = AsyncMock(side_effect=RuntimeError("embedding failed"))
        provider.get_model_name.return_value = "test-model"

        # Use the REAL _process_single_document (no replacement)
        result = await processor._process_online_batch()
        assert result is True

        _flush_exporter()
        spans = span_exporter.get_finished_spans()
        span_exporter.clear()

        consumer = _find_span(spans, "embedding_queue process", SpanKind.CONSUMER)
        assert consumer is not None, "CONSUMER span must be exported"

        # The embedding exception path sets had_failure -> ERROR
        assert consumer.status is not None
        assert consumer.status.status_code == StatusCode.ERROR, (
            "span must have ERROR status when embedding provider raises"
        )
