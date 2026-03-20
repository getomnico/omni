"""Integration tests for document-level permission enforcement.

Verifies that DocumentToolHandler and ConnectorToolHandler gate access
based on the document's permissions JSONB, using a real ParadeDB instance.
"""

import json
from unittest.mock import AsyncMock

import pytest
from ulid import ULID

from db import UsersRepository
from db.documents import DocumentsRepository
from tools.document_handler import DocumentToolHandler
from tools.connector_handler import ConnectorToolHandler
from tools.registry import ToolContext, ToolResult

pytestmark = pytest.mark.integration


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


async def _ensure_user(db_pool) -> str:
    """Create a test user and return its ID."""
    users_repo = UsersRepository(pool=db_pool)
    user = await users_repo.create(
        email=f"{ULID()}@test.local",
        password_hash="not-a-real-hash",
        full_name="Test User",
    )
    return user.id


async def _insert_source(
    db_pool, source_id: str, created_by: str, source_type: str = "google_drive"
):
    async with db_pool.acquire() as conn:
        await conn.execute(
            """INSERT INTO sources (id, source_type, name, is_active, is_deleted, created_by)
               VALUES ($1, $2, $3, true, false, $4)
               ON CONFLICT (id) DO NOTHING""",
            source_id,
            source_type,
            "Test Source",
            created_by,
        )


async def _insert_document(
    db_pool,
    doc_id: str,
    source_id: str,
    permissions: dict,
    content_type: str = "text/plain",
    external_id: str | None = None,
):
    async with db_pool.acquire() as conn:
        await conn.execute(
            """INSERT INTO documents (id, source_id, external_id, title, content_type, permissions)
               VALUES ($1, $2, $3, $4, $5, $6::jsonb)
               ON CONFLICT (id) DO NOTHING""",
            doc_id,
            source_id,
            external_id or f"ext_{doc_id}",
            f"Test Document {doc_id}",
            content_type,
            json.dumps(permissions),
        )


def _make_context(user_email: str | None, skip: bool = False) -> ToolContext:
    return ToolContext(
        chat_id="test-chat",
        user_id="test-user",
        user_email=user_email,
        skip_permission_check=skip,
    )


# ---------------------------------------------------------------------------
# DocumentToolHandler permission tests
# ---------------------------------------------------------------------------


@pytest.fixture
async def test_user_id(db_pool) -> str:
    return await _ensure_user(db_pool)


@pytest.fixture
def doc_handler(db_pool):
    """DocumentToolHandler wired to real DB, no sandbox or connector-manager."""
    mock_storage = AsyncMock()
    mock_storage.get_text.return_value = "Hello, world!"
    return DocumentToolHandler(
        content_storage=mock_storage,
        documents_repo=DocumentsRepository(db_pool),
    )


class TestDocumentHandlerPermissions:
    @pytest.mark.asyncio
    async def test_user_with_access_can_read(self, db_pool, doc_handler, test_user_id):
        source_id = str(ULID())
        doc_id = str(ULID())
        await _insert_source(db_pool, source_id, test_user_id)
        await _insert_document(
            db_pool,
            doc_id,
            source_id,
            permissions={"public": False, "users": ["alice@co.com"], "groups": []},
        )

        result = await doc_handler.execute(
            "read_document",
            {"id": doc_id, "name": "test.txt"},
            _make_context("alice@co.com"),
        )
        # Permission check passed — may fail downstream (no content_id) but NOT "Access denied"
        assert "Access denied" not in result.content[0]["text"]

    @pytest.mark.asyncio
    async def test_user_without_access_denied(self, db_pool, doc_handler, test_user_id):
        source_id = str(ULID())
        doc_id = str(ULID())
        await _insert_source(db_pool, source_id, test_user_id)
        await _insert_document(
            db_pool,
            doc_id,
            source_id,
            permissions={"public": False, "users": ["alice@co.com"], "groups": []},
        )

        result = await doc_handler.execute(
            "read_document",
            {"id": doc_id, "name": "test.txt"},
            _make_context("bob@co.com"),
        )
        assert result.is_error
        assert "Access denied" in result.content[0]["text"]

    @pytest.mark.asyncio
    async def test_public_document_accessible_to_all(
        self, db_pool, doc_handler, test_user_id
    ):
        source_id = str(ULID())
        doc_id = str(ULID())
        await _insert_source(db_pool, source_id, test_user_id)
        await _insert_document(
            db_pool,
            doc_id,
            source_id,
            permissions={"public": True, "users": [], "groups": []},
        )

        result = await doc_handler.execute(
            "read_document",
            {"id": doc_id, "name": "test.txt"},
            _make_context("anyone@co.com"),
        )
        assert "Access denied" not in result.content[0]["text"]

    @pytest.mark.asyncio
    async def test_group_access_works(self, db_pool, doc_handler, test_user_id):
        source_id = str(ULID())
        doc_id = str(ULID())
        await _insert_source(db_pool, source_id, test_user_id)
        await _insert_document(
            db_pool,
            doc_id,
            source_id,
            permissions={"public": False, "users": [], "groups": ["eng@co.com"]},
        )

        result = await doc_handler.execute(
            "read_document",
            {"id": doc_id, "name": "test.txt"},
            _make_context("eng@co.com"),
        )
        assert "Access denied" not in result.content[0]["text"]

    @pytest.mark.asyncio
    async def test_skip_permission_check_bypasses(
        self, db_pool, doc_handler, test_user_id
    ):
        source_id = str(ULID())
        doc_id = str(ULID())
        await _insert_source(db_pool, source_id, test_user_id)
        await _insert_document(
            db_pool,
            doc_id,
            source_id,
            permissions={"public": False, "users": ["alice@co.com"], "groups": []},
        )

        result = await doc_handler.execute(
            "read_document",
            {"id": doc_id, "name": "test.txt"},
            _make_context("bob@co.com", skip=True),
        )
        assert "Access denied" not in result.content[0]["text"]

    @pytest.mark.asyncio
    async def test_none_email_denied_for_private_doc(
        self, db_pool, doc_handler, test_user_id
    ):
        source_id = str(ULID())
        doc_id = str(ULID())
        await _insert_source(db_pool, source_id, test_user_id)
        await _insert_document(
            db_pool,
            doc_id,
            source_id,
            permissions={"public": False, "users": ["alice@co.com"], "groups": []},
        )

        result = await doc_handler.execute(
            "read_document",
            {"id": doc_id, "name": "test.txt"},
            _make_context(None),
        )
        assert result.is_error
        assert "Access denied" in result.content[0]["text"]


# ---------------------------------------------------------------------------
# ConnectorToolHandler permission tests (document-referencing actions)
# ---------------------------------------------------------------------------


class TestConnectorHandlerDocumentPermissions:
    @pytest.mark.asyncio
    async def test_connector_action_with_document_id_checks_permissions(
        self, db_pool, test_user_id
    ):
        source_id = str(ULID())
        doc_id = str(ULID())
        await _insert_source(db_pool, source_id, test_user_id)
        await _insert_document(
            db_pool,
            doc_id,
            source_id,
            permissions={"public": False, "users": ["alice@co.com"], "groups": []},
        )

        handler = ConnectorToolHandler(
            connector_manager_url="http://localhost:0",
            user_id="test-user",
            documents_repo=DocumentsRepository(db_pool),
        )
        # Manually register a fake action so can_handle returns True
        from tools.connector_handler import ConnectorAction

        handler._actions["google_drive__fetch_file"] = ConnectorAction(
            source_id=source_id,
            source_type="google_drive",
            source_name="Test",
            action_name="fetch_file",
            description="Fetch file",
            parameters={"document_id": {"type": "string", "required": True}},
            mode="read",
        )
        handler._initialized = True

        # Bob should be denied
        result = await handler.execute(
            "google_drive__fetch_file",
            {"document_id": doc_id},
            _make_context("bob@co.com"),
        )
        assert result.is_error
        assert "Access denied" in result.content[0]["text"]

    @pytest.mark.asyncio
    async def test_connector_action_allowed_for_permitted_user(
        self, db_pool, test_user_id
    ):
        source_id = str(ULID())
        doc_id = str(ULID())
        await _insert_source(db_pool, source_id, test_user_id)
        await _insert_document(
            db_pool,
            doc_id,
            source_id,
            permissions={"public": False, "users": ["alice@co.com"], "groups": []},
        )

        handler = ConnectorToolHandler(
            connector_manager_url="http://fake:0",
            user_id="test-user",
            documents_repo=DocumentsRepository(db_pool),
        )
        from tools.connector_handler import ConnectorAction

        handler._actions["google_drive__fetch_file"] = ConnectorAction(
            source_id=source_id,
            source_type="google_drive",
            source_name="Test",
            action_name="fetch_file",
            description="Fetch file",
            parameters={"document_id": {"type": "string", "required": True}},
            mode="read",
        )
        handler._initialized = True

        # Alice has access — the HTTP call will fail (fake URL) but the permission
        # check itself should pass, so we expect a network error, NOT "Access denied"
        result = await handler.execute(
            "google_drive__fetch_file",
            {"document_id": doc_id},
            _make_context("alice@co.com"),
        )
        assert result.is_error
        assert "Access denied" not in result.content[0]["text"]
