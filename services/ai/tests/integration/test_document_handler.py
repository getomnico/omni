"""Integration tests for DocumentToolHandler permission enforcement.

Verifies that read_document gates access based on the document's
permissions JSONB, using a real ParadeDB instance.
"""

from unittest.mock import AsyncMock

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
