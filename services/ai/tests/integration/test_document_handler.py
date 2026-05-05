"""Integration tests for DocumentToolHandler permission enforcement.

Verifies that read_document gates access based on the document's
permissions JSONB, using a real ParadeDB instance.
"""

from unittest.mock import AsyncMock, patch

import pytest
from ulid import ULID

from db.documents import DocumentsRepository
from tools.document_handler import DocumentToolHandler
from tools.registry import ToolContext
from tests.helpers import create_test_user, create_test_source, create_test_document

pytestmark = pytest.mark.integration


def _ctx(user_email: str | None, skip: bool = False) -> ToolContext:
    return ToolContext(
        chat_id="test-chat",
        user_id="test-user",
        user_email=user_email,
        skip_permission_check=skip,
    )


@pytest.fixture
async def test_user_id(db_pool) -> str:
    user_id, _ = await create_test_user(db_pool)
    return user_id


@pytest.fixture
def doc_handler(db_pool):
    mock_storage = AsyncMock()
    mock_storage.get_text.return_value = "Hello, world!"
    return DocumentToolHandler(
        content_storage=mock_storage,
        documents_repo=DocumentsRepository(db_pool),
    )


class TestDocumentHandlerPermissions:
    @pytest.mark.asyncio
    async def test_user_with_access_can_read(self, db_pool, doc_handler, test_user_id):
        source_id = await create_test_source(db_pool, test_user_id, "google_drive")
        doc_id = await create_test_document(
            db_pool,
            source_id,
            "secret.txt",
            "content",
            permissions={"public": False, "users": ["alice@co.com"], "groups": []},
        )

        result = await doc_handler.execute(
            "read_document",
            {"id": doc_id, "name": "secret.txt"},
            _ctx("alice@co.com"),
        )
        assert "not found" not in result.content[0]["text"].lower()

    @pytest.mark.asyncio
    async def test_user_without_access_denied(self, db_pool, doc_handler, test_user_id):
        source_id = await create_test_source(db_pool, test_user_id, "google_drive")
        doc_id = await create_test_document(
            db_pool,
            source_id,
            "secret.txt",
            "content",
            permissions={"public": False, "users": ["alice@co.com"], "groups": []},
        )

        result = await doc_handler.execute(
            "read_document",
            {"id": doc_id, "name": "secret.txt"},
            _ctx("bob@co.com"),
        )
        assert result.is_error
        assert "not found" in result.content[0]["text"].lower()

    @pytest.mark.asyncio
    async def test_public_document_accessible_to_all(
        self, db_pool, doc_handler, test_user_id
    ):
        source_id = await create_test_source(db_pool, test_user_id, "google_drive")
        doc_id = await create_test_document(
            db_pool,
            source_id,
            "public.txt",
            "content",
            permissions={"public": True, "users": [], "groups": []},
        )

        result = await doc_handler.execute(
            "read_document",
            {"id": doc_id, "name": "public.txt"},
            _ctx("anyone@co.com"),
        )
        assert "not found" not in result.content[0]["text"].lower()

    @pytest.mark.asyncio
    async def test_group_access_works(self, db_pool, doc_handler, test_user_id):
        source_id = await create_test_source(db_pool, test_user_id, "google_drive")
        doc_id = await create_test_document(
            db_pool,
            source_id,
            "eng-only.txt",
            "content",
            permissions={"public": False, "users": [], "groups": ["eng@co.com"]},
        )

        result = await doc_handler.execute(
            "read_document",
            {"id": doc_id, "name": "eng-only.txt"},
            _ctx("eng@co.com"),
        )
        assert "not found" not in result.content[0]["text"].lower()

    @pytest.mark.asyncio
    async def test_skip_permission_check_bypasses(
        self, db_pool, doc_handler, test_user_id
    ):
        source_id = await create_test_source(db_pool, test_user_id, "google_drive")
        doc_id = await create_test_document(
            db_pool,
            source_id,
            "secret.txt",
            "content",
            permissions={"public": False, "users": ["alice@co.com"], "groups": []},
        )

        result = await doc_handler.execute(
            "read_document",
            {"id": doc_id, "name": "secret.txt"},
            _ctx("bob@co.com", skip=True),
        )
        assert "not found" not in result.content[0]["text"].lower()


class TestPdfReturnsExtractedText:
    """`read_document` on a PDF should return the indexed extracted text rather
    than fetching the binary from the source connector. Guarded by removing
    "pdf"/"application/pdf" from BINARY_CONTENT_TYPES so the dispatch falls
    through to _read_text."""

    @pytest.mark.asyncio
    async def test_pdf_with_content_id_returns_text(self, db_pool, test_user_id):
        source_id = await create_test_source(db_pool, test_user_id, "gmail")
        content_id = str(ULID())
        doc_id = await create_test_document(
            db_pool,
            source_id,
            "report.pdf",
            "Q3 revenue grew 14% YoY.",
            permissions={"public": True, "users": [], "groups": []},
            content_type="pdf",
            content_id=content_id,
        )

        # Real content storage is mocked, but we wire in connector-manager and
        # sandbox URLs that *would* be used if PDF were treated as binary.
        # If the dispatch ever regresses, the connector-manager mock raises so
        # the test fails loudly rather than silently fetching a binary.
        mock_storage = AsyncMock()
        mock_storage.get_text.return_value = "Q3 revenue grew 14% YoY."

        handler = DocumentToolHandler(
            content_storage=mock_storage,
            documents_repo=DocumentsRepository(db_pool),
            sandbox_url="http://sandbox.invalid",
            connector_manager_url="http://connector-manager.invalid",
        )

        with patch("tools.document_handler.httpx.AsyncClient") as mock_client_cls:
            mock_client_cls.side_effect = AssertionError(
                "read_document should not have called the connector for a PDF "
                "with indexed content"
            )

            result = await handler.execute(
                "read_document",
                {"id": doc_id, "name": "report.pdf"},
                _ctx("anyone@co.com"),
            )

        assert not result.is_error
        assert result.content[0]["text"] == "Q3 revenue grew 14% YoY."
        mock_storage.get_text.assert_awaited_once_with(content_id)


class TestReadDocumentByExternalId:
    """`read_document` accepts either a documents.id (ULID) or a connector-native
    external_id. This is the path used when the model follows an attachment
    pointer from an email thread's metadata.extra.attachments — the pointer's
    `id` is the composite external_id, not the indexer-assigned ULID."""

    @pytest.mark.asyncio
    async def test_external_id_resolves_to_document(self, db_pool, test_user_id):
        source_id = await create_test_source(db_pool, test_user_id, "gmail")
        content_id = str(ULID())
        composite = "thread1:att:msg1:att1"

        doc_id = await create_test_document(
            db_pool,
            source_id,
            "report.pdf",
            "Q3 revenue grew 14% YoY.",
            permissions={"public": True, "users": [], "groups": []},
            content_type="pdf",
            content_id=content_id,
            external_id=composite,
        )

        mock_storage = AsyncMock()
        mock_storage.get_text.return_value = "Q3 revenue grew 14% YoY."

        handler = DocumentToolHandler(
            content_storage=mock_storage,
            documents_repo=DocumentsRepository(db_pool),
        )

        result = await handler.execute(
            "read_document",
            {"id": composite, "name": "report.pdf"},
            _ctx("anyone@co.com"),
        )

        assert not result.is_error
        assert result.content[0]["text"] == "Q3 revenue grew 14% YoY."
        mock_storage.get_text.assert_awaited_once_with(content_id)
        assert doc_id  # sanity: the doc was actually inserted under a real ULID

    @pytest.mark.asyncio
    async def test_external_id_respects_permissions(self, db_pool, test_user_id):
        """A user without permission on the document gets the same not-found
        response whether they query by id or external_id."""
        source_id = await create_test_source(db_pool, test_user_id, "gmail")
        composite = "thread2:att:msg2:att2"
        await create_test_document(
            db_pool,
            source_id,
            "secret.pdf",
            "secret contents",
            permissions={"public": False, "users": ["alice@co.com"], "groups": []},
            content_type="pdf",
            content_id=str(ULID()),
            external_id=composite,
        )

        handler = DocumentToolHandler(
            content_storage=AsyncMock(),
            documents_repo=DocumentsRepository(db_pool),
        )

        result = await handler.execute(
            "read_document",
            {"id": composite, "name": "secret.pdf"},
            _ctx("bob@co.com"),
        )

        assert result.is_error
        assert "not found" in result.content[0]["text"].lower()
