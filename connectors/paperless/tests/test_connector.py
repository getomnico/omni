"""Unit tests for the PaperlessConnector sync logic."""

from datetime import datetime, timezone
from typing import Any
from unittest.mock import AsyncMock, MagicMock, patch

import pytest

from paperless_connector.client import AuthenticationError, PaperlessError
from paperless_connector.connector import PaperlessConnector
from paperless_connector.models import PaperlessDocument


# ── Helpers ──────────────────────────────────────────────────────────────────


def _make_ctx(
    sync_mode: str = "full",
    state: dict[str, Any] | None = None,
    source_id: str = "src-1",
) -> MagicMock:
    """Return a mock SyncContext that records emitted documents and sync outcomes."""
    ctx = MagicMock()
    ctx.sync_mode = sync_mode
    ctx.source_id = source_id
    ctx.state = state or {}
    ctx.documents_scanned = 0
    ctx.documents_emitted = 0
    ctx.is_cancelled = MagicMock(return_value=False)

    emitted: list[Any] = []
    errors: list[tuple[str, str]] = []
    ctx._emitted = emitted
    ctx._errors = errors
    ctx._failed: list[str] = []
    ctx._completed: list[dict | None] = []
    ctx._saved_states: list[dict] = []

    async def _emit(doc: Any) -> None:
        emitted.append(doc)
        ctx.documents_emitted += 1

    async def _increment_scanned() -> None:
        ctx.documents_scanned += 1

    async def _emit_error(eid: str, msg: str) -> None:
        errors.append((eid, msg))

    async def _fail(msg: str) -> None:
        ctx._failed.append(msg)

    async def _complete(new_state: dict | None = None) -> None:
        ctx._completed.append(new_state)

    async def _save_state(state: dict) -> None:
        ctx._saved_states.append(state)

    # Content storage returns a predictable content_id
    ctx.content_storage = MagicMock()
    ctx.content_storage.save = AsyncMock(return_value="content-id-123")

    ctx.emit = AsyncMock(side_effect=_emit)
    ctx.increment_scanned = AsyncMock(side_effect=_increment_scanned)
    ctx.emit_error = AsyncMock(side_effect=_emit_error)
    ctx.fail = AsyncMock(side_effect=_fail)
    ctx.complete = AsyncMock(side_effect=_complete)
    ctx.save_state = AsyncMock(side_effect=_save_state)

    return ctx


def _raw_doc(doc_id: int = 1, title: str = "Test Doc") -> dict[str, Any]:
    return {
        "id": doc_id,
        "title": title,
        "content": "OCR text content",
        "created": "2024-01-15T10:00:00Z",
        "added": "2024-01-16T08:00:00Z",
        "modified": "2024-01-17T09:00:00Z",
        "original_file_name": "test.pdf",
        "archived_file_name": "test.pdf",
        "correspondent": None,
        "document_type": None,
        "tags": [],
        "custom_fields": [],
    }


def _parsed_doc(doc_id: int = 1, title: str = "Test Doc") -> PaperlessDocument:
    return PaperlessDocument(
        id=doc_id,
        title=title,
        content="OCR text content",
        created=datetime(2024, 1, 15, tzinfo=timezone.utc),
        added=datetime(2024, 1, 16, tzinfo=timezone.utc),
        modified=datetime(2024, 1, 17, tzinfo=timezone.utc),
        original_file_name="test.pdf",
        archived_file_name="test.pdf",
        correspondent_id=None,
        document_type_id=None,
    )


VALID_CONFIG = {"base_url": "http://paperless.local"}
VALID_CREDENTIALS = {"api_key": "token-abc"}


# ── Connector validation ─────────────────────────────────────────────────────


class TestConnectorProperties:
    def test_name(self) -> None:
        assert PaperlessConnector().name == "paperless_ngx"

    def test_source_types(self) -> None:
        assert PaperlessConnector().source_types == ["paperless_ngx"]

    def test_sync_modes(self) -> None:
        modes = PaperlessConnector().sync_modes
        assert "full" in modes
        assert "incremental" in modes


class TestConnectorConfigValidation:
    async def test_missing_base_url_fails(self) -> None:
        connector = PaperlessConnector()
        ctx = _make_ctx()
        await connector.sync({}, VALID_CREDENTIALS, None, ctx)
        assert ctx._failed
        assert "base_url" in ctx._failed[0]

    async def test_missing_api_key_fails(self) -> None:
        connector = PaperlessConnector()
        ctx = _make_ctx()
        await connector.sync(VALID_CONFIG, {}, None, ctx)
        assert ctx._failed
        assert "api_key" in ctx._failed[0]


# ── Sync behaviour ───────────────────────────────────────────────────────────


class TestFullSync:
    async def test_full_sync_emits_documents(self) -> None:
        connector = PaperlessConnector()
        ctx = _make_ctx(sync_mode="full")
        raw_docs = [_raw_doc(1, "Doc A"), _raw_doc(2, "Doc B")]

        with (
            patch(
                "paperless_connector.connector.PaperlessClient.validate",
                new_callable=AsyncMock,
            ),
            patch(
                "paperless_connector.connector.PaperlessClient.list_documents",
                new_callable=AsyncMock,
                return_value=raw_docs,
            ),
            patch(
                "paperless_connector.connector.PaperlessClient.parse_document",
                new_callable=AsyncMock,
                side_effect=lambda r: _parsed_doc(r["id"], r["title"]),
            ),
        ):
            await connector.sync(VALID_CONFIG, VALID_CREDENTIALS, None, ctx)

        assert len(ctx._emitted) == 2
        assert ctx._completed, "sync should complete successfully"

    async def test_full_sync_does_not_pass_modified_after(self) -> None:
        """Full sync should fetch all documents regardless of modification date."""
        connector = PaperlessConnector()
        ctx = _make_ctx(sync_mode="full", state={"last_sync_at": "2024-01-01T00:00:00+00:00"})

        with (
            patch(
                "paperless_connector.connector.PaperlessClient.validate",
                new_callable=AsyncMock,
            ),
            patch(
                "paperless_connector.connector.PaperlessClient.list_documents",
                new_callable=AsyncMock,
                return_value=[],
            ) as mock_list,
            patch(
                "paperless_connector.connector.PaperlessClient.parse_document",
                new_callable=AsyncMock,
            ),
        ):
            await connector.sync(VALID_CONFIG, VALID_CREDENTIALS, None, ctx)

        # For a full sync, modified_after should be None
        mock_list.assert_called_once_with(modified_after=None)

    async def test_documents_scanned_count_incremented(self) -> None:
        connector = PaperlessConnector()
        ctx = _make_ctx()
        raw_docs = [_raw_doc(1), _raw_doc(2), _raw_doc(3)]

        with (
            patch(
                "paperless_connector.connector.PaperlessClient.validate",
                new_callable=AsyncMock,
            ),
            patch(
                "paperless_connector.connector.PaperlessClient.list_documents",
                new_callable=AsyncMock,
                return_value=raw_docs,
            ),
            patch(
                "paperless_connector.connector.PaperlessClient.parse_document",
                new_callable=AsyncMock,
                side_effect=lambda r: _parsed_doc(r["id"]),
            ),
        ):
            await connector.sync(VALID_CONFIG, VALID_CREDENTIALS, None, ctx)

        assert ctx.documents_scanned == 3

    async def test_state_saved_after_completion(self) -> None:
        connector = PaperlessConnector()
        ctx = _make_ctx()

        with (
            patch(
                "paperless_connector.connector.PaperlessClient.validate",
                new_callable=AsyncMock,
            ),
            patch(
                "paperless_connector.connector.PaperlessClient.list_documents",
                new_callable=AsyncMock,
                return_value=[_raw_doc(1)],
            ),
            patch(
                "paperless_connector.connector.PaperlessClient.parse_document",
                new_callable=AsyncMock,
                return_value=_parsed_doc(1),
            ),
        ):
            await connector.sync(VALID_CONFIG, VALID_CREDENTIALS, None, ctx)

        assert ctx._completed
        final_state = ctx._completed[0]
        assert final_state is not None
        assert "last_sync_at" in final_state

    async def test_document_processing_error_continues_sync(self) -> None:
        """A single bad document should not abort the entire sync."""
        connector = PaperlessConnector()
        ctx = _make_ctx()
        raw_docs = [_raw_doc(1, "Good"), _raw_doc(2, "Bad"), _raw_doc(3, "Also Good")]

        def _parse_side_effect(raw: dict[str, Any]) -> PaperlessDocument:
            if raw["id"] == 2:
                raise ValueError("Simulated parsing error")
            return _parsed_doc(raw["id"], raw["title"])

        with (
            patch(
                "paperless_connector.connector.PaperlessClient.validate",
                new_callable=AsyncMock,
            ),
            patch(
                "paperless_connector.connector.PaperlessClient.list_documents",
                new_callable=AsyncMock,
                return_value=raw_docs,
            ),
            patch(
                "paperless_connector.connector.PaperlessClient.parse_document",
                new_callable=AsyncMock,
                side_effect=_parse_side_effect,
            ),
        ):
            await connector.sync(VALID_CONFIG, VALID_CREDENTIALS, None, ctx)

        # 2 good documents emitted, 1 error reported
        assert len(ctx._emitted) == 2
        assert len(ctx._errors) == 1
        assert ctx._completed, "sync should complete despite one error"


class TestIncrementalSync:
    async def test_incremental_passes_modified_after(self) -> None:
        connector = PaperlessConnector()
        state = {"last_sync_at": "2024-06-01T00:00:00+00:00"}
        ctx = _make_ctx(sync_mode="incremental", state=state)

        with (
            patch(
                "paperless_connector.connector.PaperlessClient.validate",
                new_callable=AsyncMock,
            ),
            patch(
                "paperless_connector.connector.PaperlessClient.list_documents",
                new_callable=AsyncMock,
                return_value=[],
            ) as mock_list,
            patch(
                "paperless_connector.connector.PaperlessClient.parse_document",
                new_callable=AsyncMock,
            ),
        ):
            await connector.sync(VALID_CONFIG, VALID_CREDENTIALS, state, ctx)

        call_kwargs = mock_list.call_args
        modified_after = call_kwargs.kwargs.get("modified_after")
        assert modified_after is not None
        assert modified_after.year == 2024
        assert modified_after.month == 6

    async def test_incremental_without_state_fetches_all(self) -> None:
        """Incremental sync with no prior state behaves like full sync."""
        connector = PaperlessConnector()
        ctx = _make_ctx(sync_mode="incremental", state=None)

        with (
            patch(
                "paperless_connector.connector.PaperlessClient.validate",
                new_callable=AsyncMock,
            ),
            patch(
                "paperless_connector.connector.PaperlessClient.list_documents",
                new_callable=AsyncMock,
                return_value=[],
            ) as mock_list,
            patch(
                "paperless_connector.connector.PaperlessClient.parse_document",
                new_callable=AsyncMock,
            ),
        ):
            await connector.sync(VALID_CONFIG, VALID_CREDENTIALS, None, ctx)

        mock_list.assert_called_once_with(modified_after=None)


class TestAuthFailures:
    async def test_authentication_error_during_validate(self) -> None:
        connector = PaperlessConnector()
        ctx = _make_ctx()

        with patch(
            "paperless_connector.connector.PaperlessClient.validate",
            new_callable=AsyncMock,
            side_effect=AuthenticationError("Invalid token"),
        ):
            await connector.sync(VALID_CONFIG, VALID_CREDENTIALS, None, ctx)

        assert ctx._failed
        assert "Authentication" in ctx._failed[0]

    async def test_connection_error_during_validate(self) -> None:
        connector = PaperlessConnector()
        ctx = _make_ctx()

        with patch(
            "paperless_connector.connector.PaperlessClient.validate",
            new_callable=AsyncMock,
            side_effect=PaperlessError("Connection refused"),
        ):
            await connector.sync(VALID_CONFIG, VALID_CREDENTIALS, None, ctx)

        assert ctx._failed
        assert "Connection" in ctx._failed[0]

    async def test_auth_error_during_document_listing(self) -> None:
        connector = PaperlessConnector()
        ctx = _make_ctx()

        with (
            patch(
                "paperless_connector.connector.PaperlessClient.validate",
                new_callable=AsyncMock,
            ),
            patch(
                "paperless_connector.connector.PaperlessClient.list_documents",
                new_callable=AsyncMock,
                side_effect=AuthenticationError("Token expired"),
            ),
        ):
            await connector.sync(VALID_CONFIG, VALID_CREDENTIALS, None, ctx)

        assert ctx._failed


class TestCancellation:
    async def test_cancelled_mid_sync_stops_processing(self) -> None:
        connector = PaperlessConnector()
        ctx = _make_ctx()
        raw_docs = [_raw_doc(i) for i in range(1, 6)]

        call_count = 0

        def _cancel_after_first(v: bool = False) -> bool:
            nonlocal call_count
            call_count += 1
            return call_count > 1  # Cancel after first document

        ctx.is_cancelled = MagicMock(side_effect=_cancel_after_first)

        with (
            patch(
                "paperless_connector.connector.PaperlessClient.validate",
                new_callable=AsyncMock,
            ),
            patch(
                "paperless_connector.connector.PaperlessClient.list_documents",
                new_callable=AsyncMock,
                return_value=raw_docs,
            ),
            patch(
                "paperless_connector.connector.PaperlessClient.parse_document",
                new_callable=AsyncMock,
                side_effect=lambda r: _parsed_doc(r["id"]),
            ),
        ):
            await connector.sync(VALID_CONFIG, VALID_CREDENTIALS, None, ctx)

        # Should have processed fewer than all 5 documents
        assert ctx.documents_scanned < 5
        assert ctx._failed, "Cancelled sync should call ctx.fail"
