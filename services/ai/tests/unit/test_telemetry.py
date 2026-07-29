"""Unit tests for telemetry helpers and embedding metric recording.

Tests use monkeypatching of production instruments/globals to avoid
contacting a real collector.  No real OTLP endpoint is required.
"""

from unittest.mock import AsyncMock, MagicMock, patch

import pytest

from telemetry import normalize_otlp_endpoint, parse_metric_export_interval
from db.documents import Document
from db.embedding_queue import EmbeddingQueueItem


# ---------------------------------------------------------------------------
# Pure helper: normalize_otlp_endpoint
# ---------------------------------------------------------------------------


class TestNormalizeOtlpEndpoint:
    def test_trailing_slash_is_stripped(self):
        assert normalize_otlp_endpoint("http://localhost:4318/") == "http://localhost:4318"

    def test_no_trailing_slash(self):
        assert normalize_otlp_endpoint("http://localhost:4318") == "http://localhost:4318"

    def test_empty_string_is_none(self):
        assert normalize_otlp_endpoint("") is None

    def test_none_is_none(self):
        assert normalize_otlp_endpoint(None) is None


# ---------------------------------------------------------------------------
# Pure helper: parse_metric_export_interval
# ---------------------------------------------------------------------------


class TestParseMetricExportInterval:
    @patch.dict("os.environ", {"OTEL_METRIC_EXPORT_INTERVAL": "30000"})
    def test_valid_positive_int(self):
        assert parse_metric_export_interval() == 30000

    @patch.dict("os.environ", {"OTEL_METRIC_EXPORT_INTERVAL": "30000"})
    def test_parses_env_var(self):
        assert parse_metric_export_interval() == 30000

    @patch.dict("os.environ", {"OTEL_METRIC_EXPORT_INTERVAL": "not-a-number"})
    def test_invalid_falls_back(self):
        assert parse_metric_export_interval() == 60_000

    @patch.dict("os.environ", {"OTEL_METRIC_EXPORT_INTERVAL": "0"})
    def test_zero_falls_back(self):
        assert parse_metric_export_interval() == 60_000

    @patch.dict("os.environ", {"OTEL_METRIC_EXPORT_INTERVAL": "-1"})
    def test_negative_falls_back(self):
        assert parse_metric_export_interval() == 60_000

    @patch.dict("os.environ", {"OTEL_METRIC_EXPORT_INTERVAL": ""})
    def test_empty_string_falls_back(self):
        assert parse_metric_export_interval() == 60_000

    @patch.dict("os.environ", clear=True)
    def test_unset_falls_back(self):
        assert parse_metric_export_interval() == 60_000


# ---------------------------------------------------------------------------
# Embedding metric recording (monkeypatch production instruments)
# ---------------------------------------------------------------------------


class TestEmbeddingMetrics:
    """Tests that exercise the real recording helpers by monkeypatching the
    global instrument references."""

    @pytest.fixture(autouse=True)
    def _patch_instruments(self):
        """Replace production instrument globals with MagicMock spies before
        each test."""
        import embeddings.batch_processor as bp

        self._patches = []
        for name in ("_EMBEDDING_PROCESSED", "_EMBEDDING_FAILED", "_EMBEDDING_BATCH_DURATION", "_EMBEDDING_PENDING"):
            mock = MagicMock()
            setattr(bp, name, mock)
            self._patches.append((name, mock))
        yield
        # Restore
        for name, _ in self._patches:
            setattr(bp, name, None)

    @property
    def _processed(self):
        return dict(self._patches)["_EMBEDDING_PROCESSED"]

    @property
    def _failed(self):
        return dict(self._patches)["_EMBEDDING_FAILED"]

    @property
    def _batch_duration(self):
        return dict(self._patches)["_EMBEDDING_BATCH_DURATION"]

    @property
    def _pending(self):
        return dict(self._patches)["_EMBEDDING_PENDING"]

    def _rerecord_helper(self):
        import embeddings.batch_processor as bp
        bp._ensure_embedding_instruments()

    def test_record_embedding_processed(self):
        from embeddings.batch_processor import _record_embedding_processed

        _record_embedding_processed()
        self._processed.add.assert_called_once_with(1)

    def test_record_embedding_processed_bulk_clone(self):
        from embeddings.batch_processor import _record_embedding_processed

        # Simulate bulk-clone path: 3 cloned documents
        _record_embedding_processed(3)
        self._processed.add.assert_called_once_with(3)

    def test_record_embedding_failed(self):
        from embeddings.batch_processor import _record_embedding_failed

        _record_embedding_failed()
        self._failed.add.assert_called_once_with(1)

    def test_record_embedding_batch_duration(self):
        from embeddings.batch_processor import _record_embedding_batch_duration

        _record_embedding_batch_duration(1.5)
        self._batch_duration.record.assert_called_once_with(1.5)

    def test_record_embedding_pending(self):
        from embeddings.batch_processor import _record_embedding_pending

        _record_embedding_pending(42)
        self._pending.set.assert_called_once_with(42)


# ---------------------------------------------------------------------------
# Bulk-clone integration path test
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_clone_same_content_embeddings_calls_processed_helper():
    """Invoke _clone_same_content_embeddings on a minimally constructed
    processor and verify the production _record_embedding_processed helper
    is called with the durable clone count and cloned items are removed."""
    import embeddings.batch_processor as bp

    # Stub instrument globals so _record_embedding_processed is observable
    processed_mock = MagicMock()
    bp._EMBEDDING_PROCESSED = processed_mock

    # Stub repos and provider
    docs_repo = AsyncMock()
    queue_repo = AsyncMock()
    embeddings_repo = AsyncMock()
    app_state = AsyncMock()
    provider = MagicMock()
    provider.get_model_name.return_value = "test-model"
    app_state.embedding_provider = provider

    processor = bp.EmbeddingBatchProcessor(
        documents_repo=docs_repo,
        queue_repo=queue_repo,
        embeddings_repo=embeddings_repo,
        app_state=app_state,
    )

    # Two queue items: one for a clonable document, one for a non-clonable
    clonable_item = EmbeddingQueueItem(
        id="item-1", document_id="doc-a", status="pending",
        error_message=None, retry_count=0, created_at=None,
    )
    nonclonable_item = EmbeddingQueueItem(
        id="item-2", document_id="doc-b", status="pending",
        error_message=None, retry_count=0, created_at=None,
    )
    items = [clonable_item, nonclonable_item]

    # documents_by_id: doc-a has content_id (can clone), doc-b has no content_id
    doc_a = Document(id="doc-a", content_id="content-1")
    doc_b = Document(id="doc-b", content_id=None)
    documents_by_id = {"doc-a": doc_a, "doc-b": doc_b}

    # find_embedded_content_donors returns a donor for content-1
    docs_repo.find_embedded_content_donors.return_value = {"content-1": "donor-doc"}

    # bulk_clone_for_documents returns one clone
    embeddings_repo.bulk_clone_for_documents.return_value = {"doc-a": 3}

    result = await processor._clone_same_content_embeddings(items, documents_by_id)

    # Verify _record_embedding_processed was called with the clone count
    assert processed_mock.add.call_count >= 1
    # _record_embedding_processed is called with len(clone_counts) = 1 document
    assert processed_mock.add.call_args.args[0] == 1

    # Verify cloned items are removed from the returned list
    returned_ids = [item.document_id for item in result]
    assert "doc-a" not in returned_ids
    assert "doc-b" in returned_ids
    assert len(result) == 1
