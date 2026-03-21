"""Integration tests for ConnectorToolHandler permission enforcement.

Verifies that connector actions referencing a document_id gate access
based on the document's permissions JSONB, using a real ParadeDB instance.
"""

import pytest

from db.documents import DocumentsRepository
from tools.connector_handler import ConnectorAction, ConnectorToolHandler
from tools.registry import ToolContext
from tests.helpers import create_test_user, create_test_source, create_test_document

pytestmark = pytest.mark.integration


def _ctx(user_email: str | None) -> ToolContext:
    return ToolContext(
        chat_id="test-chat",
        user_id="test-user",
        user_email=user_email,
    )


def _register_fetch_file(handler: ConnectorToolHandler, source_id: str) -> None:
    """Register a fake fetch_file action so can_handle returns True."""
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


@pytest.fixture
async def test_user_id(db_pool) -> str:
    user_id, _ = await create_test_user(db_pool)
    return user_id


class TestConnectorHandlerDocumentPermissions:
    @pytest.mark.asyncio
    async def test_unauthorized_user_denied(self, db_pool, test_user_id):
        source_id = await create_test_source(db_pool, test_user_id, "google_drive")
        doc_id = await create_test_document(
            db_pool,
            source_id,
            "secret.pdf",
            "content",
            permissions={"public": False, "users": ["alice@co.com"], "groups": []},
        )

        handler = ConnectorToolHandler(
            connector_manager_url="http://localhost:0",
            user_id="test-user",
            documents_repo=DocumentsRepository(db_pool),
        )
        _register_fetch_file(handler, source_id)

        result = await handler.execute(
            "google_drive__fetch_file",
            {"document_id": doc_id},
            _ctx("bob@co.com"),
        )
        assert result.is_error
        assert "not found" in result.content[0]["text"].lower()

    @pytest.mark.asyncio
    async def test_authorized_user_passes_permission_check(self, db_pool, test_user_id):
        source_id = await create_test_source(db_pool, test_user_id, "google_drive")
        doc_id = await create_test_document(
            db_pool,
            source_id,
            "secret.pdf",
            "content",
            permissions={"public": False, "users": ["alice@co.com"], "groups": []},
        )

        handler = ConnectorToolHandler(
            connector_manager_url="http://fake:0",
            user_id="test-user",
            documents_repo=DocumentsRepository(db_pool),
        )
        _register_fetch_file(handler, source_id)

        # Alice has access — the HTTP call will fail (fake URL) but the permission
        # check itself should pass, so we expect a network error, not "not found"
        result = await handler.execute(
            "google_drive__fetch_file",
            {"document_id": doc_id},
            _ctx("alice@co.com"),
        )
        assert result.is_error
        assert "not found" not in result.content[0]["text"].lower()
