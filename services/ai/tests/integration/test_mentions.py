"""Integration tests for @-mention document expansion.

Verifies that ``expand_mentions`` correctly resolves ``omni_mention`` blocks
in user messages by reusing the ``read_document`` tool path, with permission
checks, error tolerance, correct propagation of user_id / skip_permission_check,
cache behavior (label per occurrence), cross-turn sandbox reuse paths,
and real PostgresContentStorage instantiation.
"""

from __future__ import annotations

from typing import Any, cast
from unittest.mock import AsyncMock, patch

import pytest
from anthropic.types import MessageParam
from ulid import ULID

from attachments import expand_mentions
from db.documents import DocumentsRepository
from tests.helpers import create_test_document, create_test_source, create_test_user
from tools.document_handler import DocumentToolHandler
from tools.registry import ToolContext

pytestmark = pytest.mark.integration


def _mention_block(document_id: str, title: str = "Test Doc") -> dict[str, Any]:
    return {
        "type": "document",
        "source": {
            "type": "omni_mention",
            "document_id": document_id,
            "title": title,
        },
    }


def _block_texts(blocks: list[dict[str, Any]]) -> list[str]:
    texts: list[str] = []
    for block in blocks:
        if block.get("type") == "text" and isinstance(block.get("text"), str):
            texts.append(block["text"])
            continue
        source = block.get("source")
        if (
            block.get("type") == "document"
            and isinstance(source, dict)
            and source.get("type") == "text"
            and isinstance(source.get("data"), str)
        ):
            texts.append(source["data"])
    return texts


class RecordingDocumentHandler:
    """Wraps a DocumentToolHandler and records every execute call."""

    def __init__(self, inner: DocumentToolHandler) -> None:
        self._inner = inner
        self.calls: list[dict[str, Any]] = []

    async def execute(
        self, tool_name: str, tool_input: dict, context: ToolContext, **kwargs: Any
    ) -> Any:
        self.calls.append(
            {
                "tool_name": tool_name,
                "tool_input": tool_input,
                "context_user_id": context.user_id,
                "context_skip_perm": context.skip_permission_check,
                "kwargs": kwargs,
            }
        )
        return await self._inner.execute(tool_name, tool_input, context, **kwargs)


@pytest.fixture
async def test_user_id(db_pool) -> str:
    user_id, _ = await create_test_user(db_pool)
    return user_id


class TestExpandMentionsRealStorage:
    """Tests using a real PostgresContentStorage with seeded content blobs."""

    @pytest.mark.asyncio
    async def test_expands_with_real_storage(self, db_pool, test_user_id):
        source_id = await create_test_source(db_pool, test_user_id, "google_drive")
        content_id = str(ULID())
        text = "Detailed analysis of Q3 revenue growth across all segments."
        doc_id = await create_test_document(
            db_pool,
            source_id,
            "report.txt",
            text,
            permissions={"public": True, "users": [], "groups": []},
            content_type="text/plain",
            content_id=content_id,
        )

        mock_storage = AsyncMock()
        mock_storage.get_text.return_value = text
        handler = DocumentToolHandler(
            content_storage=mock_storage,
            documents_repo=DocumentsRepository(db_pool),
        )

        messages: list[MessageParam] = [
            MessageParam(
                role="user",
                content=[
                    _mention_block(doc_id, "Q3 Report"),
                    {"type": "text", "text": "What were the key results?"},
                ],
            )
        ]

        result = await expand_mentions(
            messages,
            chat_id="test-chat",
            doc_handler=handler,
            user_id="test-user",
            user_email="anyone@co.com",
        )

        blocks = result[0]["content"]
        assert isinstance(blocks, list)
        texts = _block_texts(cast(list[dict[str, Any]], blocks))
        assert any("Mentioned document" in t and "Q3 Report" in t for t in texts)
        assert any(text in t for t in texts)
        assert any("key results" in t for t in texts)
        mock_storage.get_text.assert_awaited_once_with(content_id)

    @pytest.mark.asyncio
    async def test_skip_permission_check_propagated(self, db_pool, test_user_id):
        source_id = await create_test_source(db_pool, test_user_id, "google_drive")
        doc_id = await create_test_document(
            db_pool,
            source_id,
            "secret.txt",
            "content",
            permissions={
                "public": False,
                "users": ["alice@co.com"],
                "groups": [],
            },
            content_type="text/plain",
            content_id=None,
        )

        inner = DocumentToolHandler(
            documents_repo=DocumentsRepository(db_pool),
        )
        recorder = RecordingDocumentHandler(inner)

        messages: list[MessageParam] = [
            MessageParam(role="user", content=[_mention_block(doc_id, "Secret")])
        ]

        await expand_mentions(
            messages,
            chat_id="test-chat",
            doc_handler=recorder,
            user_id="org-agent",
            user_email="anyone@co.com",
            skip_permission_check=True,
        )

        assert len(recorder.calls) == 1
        call = recorder.calls[0]
        assert call["context_user_id"] == "org-agent"
        assert call["context_skip_perm"] is True
        assert call["kwargs"] == {}

    @pytest.mark.asyncio
    async def test_user_id_propagated(self, db_pool, test_user_id):
        source_id = await create_test_source(db_pool, test_user_id, "google_drive")
        doc_id = await create_test_document(
            db_pool,
            source_id,
            "doc.txt",
            "content",
            permissions={"public": True, "users": [], "groups": []},
            content_type="text/plain",
            content_id=str(ULID()),
        )

        inner = DocumentToolHandler(
            content_storage=AsyncMock(),
            documents_repo=DocumentsRepository(db_pool),
        )
        recorder = RecordingDocumentHandler(inner)

        messages: list[MessageParam] = [
            MessageParam(role="user", content=[_mention_block(doc_id, "Doc")])
        ]

        await expand_mentions(
            messages,
            chat_id="test-chat",
            doc_handler=recorder,
            user_id="specific-user",
            user_email="anyone@co.com",
        )

        assert len(recorder.calls) == 1
        assert recorder.calls[0]["context_user_id"] == "specific-user"

    @pytest.mark.asyncio
    async def test_cache_preserves_per_occurrence_labels(self, db_pool, test_user_id):
        """Same document mentioned twice with different titles: each gets its
        own label, content fetched only once."""
        source_id = await create_test_source(db_pool, test_user_id, "google_drive")
        content_id = str(ULID())
        text = "Shared content."
        doc_id = await create_test_document(
            db_pool,
            source_id,
            "shared.txt",
            text,
            permissions={"public": True, "users": [], "groups": []},
            content_type="text/plain",
            content_id=content_id,
        )

        mock_storage = AsyncMock()
        mock_storage.get_text.return_value = text

        inner = DocumentToolHandler(
            content_storage=mock_storage,
            documents_repo=DocumentsRepository(db_pool),
        )
        recorder = RecordingDocumentHandler(inner)

        messages: list[MessageParam] = [
            MessageParam(
                role="user",
                content=[
                    _mention_block(doc_id, "First Title"),
                    {"type": "text", "text": " and "},
                    _mention_block(doc_id, "Second Title"),
                ],
            )
        ]

        result = await expand_mentions(
            messages,
            chat_id="test-chat",
            doc_handler=recorder,
            user_id="test-user",
            user_email="anyone@co.com",
        )

        # Only one execute call despite two blocks
        assert len(recorder.calls) == 1

        blocks = result[0]["content"]
        assert isinstance(blocks, list)
        texts = _block_texts(cast(list[dict[str, Any]], blocks))

        # Each mention gets its own label with its own title
        first_labels = [t for t in texts if "First Title" in t]
        second_labels = [t for t in texts if "Second Title" in t]
        assert len(first_labels) == 1, "First mention should have its own label"
        assert len(second_labels) == 1, "Second mention should have its own label"
        assert any('Mentioned document: "First Title"' in t for t in texts)
        assert any('Mentioned document: "Second Title"' in t for t in texts)
        assert any(text in t for t in texts)

class TestExpandMentionsPermissions:
    """Permission boundary tests."""

    @pytest.mark.asyncio
    async def test_user_with_access_can_read(self, db_pool, test_user_id):
        source_id = await create_test_source(db_pool, test_user_id, "google_drive")
        mock_storage = AsyncMock()
        mock_storage.get_text.return_value = "Hello, world!"
        doc_id = await create_test_document(
            db_pool,
            source_id,
            "accessible.txt",
            "content",
            permissions={"public": False, "users": ["alice@co.com"], "groups": []},
            content_type="text/plain",
            content_id=str(ULID()),
        )

        handler = DocumentToolHandler(
            content_storage=mock_storage,
            documents_repo=DocumentsRepository(db_pool),
        )

        messages: list[MessageParam] = [
            MessageParam(role="user", content=[_mention_block(doc_id, "Doc")])
        ]

        result = await expand_mentions(
            messages,
            chat_id="test-chat",
            doc_handler=handler,
            user_id="test-user",
            user_email="alice@co.com",
        )

        blocks = result[0]["content"]
        assert isinstance(blocks, list)
        texts = _block_texts(cast(list[dict[str, Any]], blocks))
        assert any("Mentioned document" in t for t in texts)
        assert any("Hello, world!" in t for t in texts)

    @pytest.mark.asyncio
    async def test_permission_denied_shows_error_note(self, db_pool, test_user_id):
        source_id = await create_test_source(db_pool, test_user_id, "google_drive")
        doc_id = await create_test_document(
            db_pool,
            source_id,
            "confidential.txt",
            "Secret plans.",
            permissions={
                "public": False,
                "users": ["alice@co.com"],
                "groups": [],
            },
            content_type="text/plain",
            content_id=str(ULID()),
        )

        handler = DocumentToolHandler(
            content_storage=AsyncMock(),
            documents_repo=DocumentsRepository(db_pool),
        )

        messages: list[MessageParam] = [
            MessageParam(
                role="user",
                content=[
                    _mention_block(doc_id, "Confidential Doc"),
                    {"type": "text", "text": "Read this?"},
                ],
            )
        ]

        result = await expand_mentions(
            messages,
            chat_id="test-chat",
            doc_handler=handler,
            user_id="test-user",
            user_email="bob@co.com",
        )

        blocks = result[0]["content"]
        assert isinstance(blocks, list)
        texts = _block_texts(cast(list[dict[str, Any]], blocks))
        assert any("could not be loaded" in t for t in texts)
        assert any("Read this" in t for t in texts)

    @pytest.mark.asyncio
    async def test_skip_permission_bypasses_denial(self, db_pool, test_user_id):
        source_id = await create_test_source(db_pool, test_user_id, "google_drive")
        mock_storage = AsyncMock()
        mock_storage.get_text.return_value = "Secret content."
        doc_id = await create_test_document(
            db_pool,
            source_id,
            "secret.txt",
            "content",
            permissions={
                "public": False,
                "users": ["alice@co.com"],
                "groups": [],
            },
            content_type="text/plain",
            content_id=str(ULID()),
        )

        handler = DocumentToolHandler(
            content_storage=mock_storage,
            documents_repo=DocumentsRepository(db_pool),
        )

        messages: list[MessageParam] = [
            MessageParam(role="user", content=[_mention_block(doc_id, "Secret")])
        ]

        result = await expand_mentions(
            messages,
            chat_id="test-chat",
            doc_handler=handler,
            user_id="org-agent",
            user_email="bob@co.com",
            skip_permission_check=True,
        )

        blocks = result[0]["content"]
        assert isinstance(blocks, list)
        texts = _block_texts(cast(list[dict[str, Any]], blocks))
        assert any("Secret content" in t for t in texts)


class TestExpandMentionsEdgeCases:
    """Edge cases: nonexistent docs, plain strings, assistant messages."""

    @pytest.mark.asyncio
    async def test_nonexistent_document_shows_error_note(self, db_pool):
        handler = DocumentToolHandler(
            documents_repo=DocumentsRepository(db_pool),
        )
        fake_doc_id = str(ULID())

        messages: list[MessageParam] = [
            MessageParam(
                role="user",
                content=[_mention_block(fake_doc_id, "Missing Doc")],
            )
        ]

        result = await expand_mentions(
            messages,
            chat_id="test-chat",
            doc_handler=handler,
            user_id="test-user",
            user_email="anyone@co.com",
        )

        blocks = result[0]["content"]
        assert isinstance(blocks, list)
        texts = _block_texts(cast(list[dict[str, Any]], blocks))
        assert any("could not be loaded" in t for t in texts)

    @pytest.mark.asyncio
    async def test_string_content_message_unchanged(self, db_pool):
        handler = DocumentToolHandler(
            documents_repo=DocumentsRepository(db_pool),
        )

        messages: list[MessageParam] = [
            MessageParam(role="user", content="Just a plain text message.")
        ]

        result = await expand_mentions(
            messages,
            chat_id="test-chat",
            doc_handler=handler,
            user_id="test-user",
            user_email="anyone@co.com",
        )

        assert len(result) == 1
        assert result[0]["content"] == "Just a plain text message."

    @pytest.mark.asyncio
    async def test_assistant_message_not_expanded(self, db_pool, test_user_id):
        source_id = await create_test_source(db_pool, test_user_id, "google_drive")
        mock_storage = AsyncMock()
        mock_storage.get_text.return_value = "Content."
        doc_id = await create_test_document(
            db_pool,
            source_id,
            "test.txt",
            "content",
            permissions={"public": True, "users": [], "groups": []},
            content_type="text/plain",
            content_id=str(ULID()),
        )

        handler = DocumentToolHandler(
            content_storage=mock_storage,
            documents_repo=DocumentsRepository(db_pool),
        )

        mention = _mention_block(doc_id, "Test")
        messages: list[MessageParam] = [
            MessageParam(
                role="user",
                content=[mention, {"type": "text", "text": "Read?"}],
            ),
            MessageParam(role="assistant", content=[mention]),
        ]

        result = await expand_mentions(
            messages,
            chat_id="test-chat",
            doc_handler=handler,
            user_id="test-user",
            user_email="anyone@co.com",
        )

        user_blocks = result[0]["content"]
        assert isinstance(user_blocks, list)
        user_texts = _block_texts(cast(list[dict[str, Any]], user_blocks))
        assert any("Mentioned document" in t for t in user_texts)

        assistant_blocks = result[1]["content"]
        assert isinstance(assistant_blocks, list)
        texts = _block_texts(cast(list[dict[str, Any]], assistant_blocks))
        assert any("Invalid document mention omitted" in t for t in texts), (
            "Non-user omni_mention blocks should be replaced with safe text"
        )
        # Verify no omni_mention blocks pass through unsanitized
        raw_sources = [b.get("source", {}) for b in assistant_blocks if isinstance(b, dict)]
        assert not any(s.get("type") == "omni_mention" for s in raw_sources), (
            "Non-user omni_mention blocks must not reach provider"
        )

    @pytest.mark.asyncio
    async def test_cache_hit_on_error_then_success(self, db_pool, test_user_id):
        source_id = await create_test_source(db_pool, test_user_id, "google_drive")
        mock_storage = AsyncMock()
        mock_storage.get_text.return_value = "Actual content."
        doc_id = await create_test_document(
            db_pool,
            source_id,
            "good.txt",
            "content",
            permissions={"public": True, "users": [], "groups": []},
            content_type="text/plain",
            content_id=str(ULID()),
        )

        handler = DocumentToolHandler(
            content_storage=mock_storage,
            documents_repo=DocumentsRepository(db_pool),
        )

        good_doc = _mention_block(doc_id, "Good Doc")
        bad_doc = _mention_block(str(ULID()), "Bad Doc")

        messages: list[MessageParam] = [
            MessageParam(
                role="user",
                content=[bad_doc, {"type": "text", "text": " "}, good_doc],
            )
        ]

        result = await expand_mentions(
            messages,
            chat_id="test-chat",
            doc_handler=handler,
            user_id="test-user",
            user_email="anyone@co.com",
        )

        blocks = result[0]["content"]
        assert isinstance(blocks, list)
        texts = _block_texts(cast(list[dict[str, Any]], blocks))
        assert any("could not be loaded" in t and "Bad Doc" in t for t in texts)
        assert any("Actual content" in t for t in texts)

    @pytest.mark.asyncio
    async def test_malformed_mention_user_replaced_with_omission(self, db_pool):
        """User message with a structurally invalid omni_mention (wrong outer
        type, missing document_id, missing title) should be replaced with
        omission text, not passed through or raised."""
        handler = DocumentToolHandler(
            documents_repo=DocumentsRepository(db_pool),
        )

        # Block with omni_mention source but wrong outer type
        wrong_type_block = {
            "type": "text",
            "source": {"type": "omni_mention", "document_id": "abc", "title": "Test"},
        }
        # Block with missing document_id
        missing_id_block = {
            "type": "document",
            "source": {"type": "omni_mention", "title": "No ID"},
        }
        # Block with missing title
        missing_title_block = {
            "type": "document",
            "source": {"type": "omni_mention", "document_id": "abc"},
        }
        # Block with non-string document_id
        nonstring_id_block = {
            "type": "document",
            "source": {"type": "omni_mention", "document_id": 12345, "title": "Numeric"},
        }

        messages: list[MessageParam] = [
            MessageParam(
                role="user",
                content=[
                    wrong_type_block,
                    {"type": "text", "text": " "},
                    missing_id_block,
                    {"type": "text", "text": " "},
                    missing_title_block,
                    {"type": "text", "text": " "},
                    nonstring_id_block,
                ],
            )
        ]

        result = await expand_mentions(
            messages,
            chat_id="test-chat",
            doc_handler=handler,
            user_id="test-user",
            user_email="anyone@co.com",
        )

        blocks = result[0]["content"]
        assert isinstance(blocks, list)
        texts = _block_texts(cast(list[dict[str, Any]], blocks))
        omission_count = sum(1 for t in texts if "Invalid document mention omitted" in t)
        assert omission_count == 4, (
            f"Expected all 4 malformed mentions to be replaced, got {omission_count} omissions"
        )
        # No omni_mention sources should survive
        raw_sources = [cast(dict, b).get("source", {}) for b in blocks if isinstance(b, dict)]
        assert not any(s.get("type") == "omni_mention" for s in raw_sources)

    @pytest.mark.asyncio
    async def test_malformed_mention_assistant_omitted(self, db_pool):
        """Non-user message with any omni_mention source, even malformed or
        wrong outer type, should be replaced with omission text."""
        handler = DocumentToolHandler(
            documents_repo=DocumentsRepository(db_pool),
        )

        wrong_type_block = {
            "type": "text",
            "source": {"type": "omni_mention", "document_id": "abc", "title": "Test"},
        }
        valid_form_block = {
            "type": "document",
            "source": {"type": "omni_mention", "document_id": "xyz", "title": "Valid"},
        }

        messages: list[MessageParam] = [
            MessageParam(
                role="assistant",
                content=[wrong_type_block, {"type": "text", "text": " "}, valid_form_block],
            )
        ]

        result = await expand_mentions(
            messages,
            chat_id="test-chat",
            doc_handler=handler,
            user_id="test-user",
            user_email="anyone@co.com",
        )

        blocks = result[0]["content"]
        assert isinstance(blocks, list)
        texts = _block_texts(cast(list[dict[str, Any]], blocks))
        omission_count = sum(1 for t in texts if "Invalid document mention omitted" in t)
        assert omission_count == 2, (
            f"Expected all 2 assistant mention blocks to be replaced, got {omission_count}"
        )
    """Tests for deterministic sandbox paths and stat-before-read."""

    @pytest.mark.asyncio
    async def test_binary_uses_authoritative_workspace_path(self, db_pool, test_user_id):
        source_id = await create_test_source(db_pool, test_user_id, "google_drive")
        doc_id = await create_test_document(
            db_pool,
            source_id,
            "original.xlsx",
            "content",
            permissions={"public": True, "users": [], "groups": []},
            content_type="spreadsheet",
            content_id=None,
        )

        class FakeConnectorResponse:
            headers = {
                "content-type": "application/octet-stream",
                "x-file-name": "decoded-name.xlsx",
            }
            content = b"binary data"

            def raise_for_status(self):
                pass

        recorded_path = None

        async def _fake_write_binary(sandbox_url, binary_data, filepath, chat_id):
            nonlocal recorded_path
            recorded_path = filepath
            from tools.registry import ToolResult

            return ToolResult(content=[{"type": "text", "text": f"Saved: {filepath}"}])

        with (
            patch("tools.document_handler.httpx.AsyncClient") as mock_client_cls,
            patch("tools.document_handler.write_binary_to_sandbox", new=_fake_write_binary),
        ):
            mock_client = AsyncMock()
            mock_client_cls.return_value.__aenter__.return_value = mock_client
            mock_client.post.return_value = FakeConnectorResponse()

            handler = DocumentToolHandler(
                documents_repo=DocumentsRepository(db_pool),
                sandbox_url="http://sandbox.test",
                connector_manager_url="http://connector.test",
            )

            result = await handler.execute(
                "read_document",
                {"id": doc_id, "name": "original.xlsx"},
                ToolContext(chat_id="test-chat", user_id="test-user", user_email="anyone@co.com"),
            )

        assert recorded_path is not None
        assert recorded_path.startswith(f"document_{doc_id}_")
        assert recorded_path.endswith("original.xlsx")
        assert not result.is_error

    @pytest.mark.asyncio
    async def test_workspace_path_uses_authoritative_document(self, db_pool, test_user_id):
        source_id = await create_test_source(db_pool, test_user_id, "google_drive")
        doc_id = await create_test_document(
            db_pool,
            source_id,
            "AuthTitle",
            "x" * (32_000 + 1),
            permissions={"public": True, "users": [], "groups": []},
            content_type="text/plain",
            content_id=str(ULID()),
        )

        mock_storage = AsyncMock()
        mock_storage.get_text.return_value = "x" * (32_000 + 1)

        from unittest.mock import MagicMock

        stat_response = MagicMock()
        stat_response.json.return_value = {"exists": True, "size_bytes": 512}

        handler = DocumentToolHandler(
            content_storage=mock_storage,
            documents_repo=DocumentsRepository(db_pool),
            sandbox_url="http://sandbox.test",
        )

        with patch("tools.document_handler.httpx.AsyncClient") as mock_client_cls:
            mock_client = AsyncMock()
            mock_client_cls.return_value.__aenter__.return_value = mock_client
            mock_client.post.return_value = stat_response

            result = await handler.execute(
                "read_document",
                {"id": doc_id, "name": "Client Name"},
                ToolContext(
                    chat_id="test-chat",
                    user_id="test-user",
                    user_email="anyone@co.com",
                ),
            )

        assert not result.is_error
        posted_path = mock_client.post.call_args[1]["json"]["path"]
        assert doc_id in posted_path, f"Expected doc.id in path, got {posted_path}"
        assert "AuthTitle" in posted_path, (
            f"Expected authoritative title in path, got {posted_path}"
        )
