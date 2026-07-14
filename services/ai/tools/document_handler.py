"""DocumentToolHandler: unified handler for reading/fetching documents."""

from __future__ import annotations

import base64
import logging
from typing import Union
from urllib.parse import unquote

import httpx
from anthropic.types import ToolParam

from db.documents import DocumentsRepository
from storage import ContentStorage, PostgresContentStorage
from tools.registry import ToolContext, ToolResult
from tools.sandbox import write_binary_to_sandbox, write_text_to_sandbox

logger = logging.getLogger(__name__)


def _safe_basename(title: str) -> str:
    """Sanitize a document title to a safe filesystem basename.

    Keeps only letters, digits, dots, dashes, underscores. Strips control
    characters and path separators. Truncates to 128 chars to stay safely
    bounded.
    """
    safe = "".join(c for c in title if c.isalnum() or c in ".-_")
    safe = safe.strip(".-")
    return safe[:128] or "document"


# Content types considered binary (not extracted text).
# The documents.content_type column stores the standardized content_type
# (e.g. "spreadsheet") when set, falling back to MIME type otherwise.
#
# PDFs are deliberately omitted: the indexer extracts their text at sync time
# and stores it in content_blobs, so read_document can return that text directly
# instead of forcing the model to download the binary and re-extract in the
# sandbox.
BINARY_CONTENT_TYPES = {
    # Standardized content types
    "spreadsheet",
    "document",
    "presentation",
    # MIME type fallbacks (for documents without a standardized content_type)
    "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
    "application/vnd.ms-excel",
    "application/vnd.google-apps.spreadsheet",
    "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
    "application/vnd.google-apps.document",
    "application/vnd.openxmlformats-officedocument.presentationml.presentation",
    "application/vnd.google-apps.presentation",
    "image/png",
    "image/jpeg",
    "image/gif",
    "image/webp",
    "application/zip",
    "application/octet-stream",
}

# Max text size to return directly in LLM context (characters)
DIRECT_RETURN_THRESHOLD = 32_000

PDF_CONTENT_TYPES = {"pdf", "application/pdf", "application/x-pdf"}
PDF_EXTRACTION_FAILURE_TEXT = (
    "[Text extraction failed for this PDF. The document was skipped "
    "for extracted-text indexing because no text could be extracted.]"
)
PDF_EXTRACTION_FAILURE_REASON_PREFIX = (
    "[Text extraction failed for this PDF. The document was skipped "
    "for extracted-text indexing. Reason: "
)


def _is_pdf_extraction_failure(content_type: str | None, content: str) -> bool:
    if content_type not in PDF_CONTENT_TYPES:
        return False
    if content == PDF_EXTRACTION_FAILURE_TEXT:
        return True
    return (
        content.startswith(PDF_EXTRACTION_FAILURE_REASON_PREFIX)
        and len(content) > len(PDF_EXTRACTION_FAILURE_REASON_PREFIX) + 1
        and content.endswith("]")
    )


DOCUMENT_TOOL = {
    "name": "read_document",
    "description": (
        "Read a document's full content. For text documents, returns content directly or saves to sandbox if large. "
        "For binary files (spreadsheets, PDFs, etc.), fetches the actual file from the source and saves to sandbox workspace. "
        "Use the [_ref:ULID] value from search_documents results as the 'id' argument."
    ),
    "input_schema": {
        "type": "object",
        "properties": {
            "id": {
                "type": "string",
                "description": "The document ID. Use the [_ref:ULID] value from search results. Never pass a filename or URL.",
            },
            "name": {
                "type": "string",
                "description": "The document name",
            },
            "start_line": {
                "type": "integer",
                "description": "Optional: start line number (inclusive) for partial text reads",
            },
            "end_line": {
                "type": "integer",
                "description": "Optional: end line number (inclusive) for partial text reads",
            },
        },
        "required": ["id", "name"],
    },
}

_TOOL_NAMES = {"read_document"}


class DocumentToolHandler:
    """Unified handler for reading text documents and fetching binary files."""

    def __init__(
        self,
        content_storage: Union[ContentStorage, PostgresContentStorage, None] = None,
        documents_repo: DocumentsRepository | None = None,
        sandbox_url: str | None = None,
        connector_manager_url: str | None = None,
    ) -> None:
        self._content_storage = content_storage
        self._documents_repo = documents_repo
        self._sandbox_url = sandbox_url.rstrip("/") if sandbox_url else None
        self._connector_manager_url = (
            connector_manager_url.rstrip("/") if connector_manager_url else None
        )

    def get_tools(self) -> list[ToolParam]:
        return [DOCUMENT_TOOL]

    def can_handle(self, tool_name: str) -> bool:
        return tool_name in _TOOL_NAMES

    def requires_approval(self, tool_name: str) -> bool:
        return False  # read-only operation

    async def execute(
        self,
        tool_name: str,
        tool_input: dict,
        context: ToolContext,
        *,
        mention_document_id: str | None = None,
    ) -> ToolResult:
        if tool_name != "read_document":
            return ToolResult(
                content=[{"type": "text", "text": f"Unknown tool: {tool_name}"}],
                is_error=True,
            )

        document_id = tool_input.get("id", "")
        if document_id and document_id.startswith("_ref:"):
            document_id = document_id[len("_ref:") :]
        document_name = tool_input.get("name", document_id)
        start_line = tool_input.get("start_line")
        end_line = tool_input.get("end_line")

        if not document_id:
            return ToolResult(
                content=[{"type": "text", "text": "Missing required parameter: id"}],
                is_error=True,
            )

        try:
            user_email = None if context.skip_permission_check else context.user_email
            doc = await self._documents_repo.get_by_id(document_id, user_email=user_email)
            if doc is None:
                doc = await self._documents_repo.get_by_external_id(
                    document_id, user_email=user_email
                )
            if doc is None:
                return ToolResult(
                    content=[
                        {
                            "type": "text",
                            "text": f"Document not found: {document_id}",
                        }
                    ],
                    is_error=True,
                )

            # When mention_document_id is set (expand_mentions path), use a
            # deterministic workspace path and stat-before-read to avoid
            # re-fetching cross-turn. The path includes the authoritative
            # doc.id and the blob content_id (when available) for freshness.
            # Docs without content_id (e.g. source-fetched binaries) skip
            # stat reuse and are always re-fetched.
            deterministic_path = None
            if mention_document_id and self._sandbox_url and doc.content_id:
                safe_title = _safe_basename(doc.title or "document")
                deterministic_path = f"mention_{doc.id}_{doc.content_id}_{safe_title}"
                stat_result = await self._stat_sandbox_path(deterministic_path, context.chat_id)
                if stat_result:
                    size_kb = stat_result.get("size_bytes", 0) / 1024
                    return ToolResult(
                        content=[
                            {
                                "type": "text",
                                "text": f"Document saved to workspace: {deterministic_path} ({size_kb:.1f} KB). Use read_file or run_python to process it.",
                            }
                        ],
                    )
            elif mention_document_id and self._sandbox_url and not doc.content_id:
                # No content_id — build a deterministic write path but skip
                # stat reuse; always re-fetch to avoid stale data.
                safe_title = _safe_basename(doc.title or "document")
                deterministic_path = f"mention_{doc.id}_{safe_title}"

            is_binary = doc.content_type in BINARY_CONTENT_TYPES

            has_extracted_text = False
            if is_binary and doc.content_id and self._content_storage:
                try:
                    meta = await self._content_storage.get_metadata(doc.content_id)
                    has_extracted_text = bool(
                        meta.content_type and meta.content_type.startswith("text/")
                    )
                except Exception:
                    logger.debug(
                        "get_metadata failed for content_id %s",
                        doc.content_id,
                        exc_info=True,
                    )

            if (
                is_binary
                and not has_extracted_text
                and self._connector_manager_url
                and doc.source_id
            ):
                return await self._fetch_binary(
                    doc, document_name, context, deterministic_path=deterministic_path
                )
            else:
                return await self._read_text(
                    doc,
                    document_name,
                    start_line,
                    end_line,
                    context,
                    deterministic_path=deterministic_path,
                )

        except Exception as e:
            logger.error(f"read_document failed: {e}", exc_info=True)
            return ToolResult(
                content=[{"type": "text", "text": f"read_document error: {e}"}],
                is_error=True,
            )

    async def _stat_sandbox_path(self, path: str, chat_id: str) -> dict | None:
        """Stat a sandbox path, returning metadata dict if it exists, None otherwise."""
        if not self._sandbox_url:
            return None
        base = self._sandbox_url.rstrip("/")
        try:
            async with httpx.AsyncClient(timeout=10.0) as client:
                resp = await client.post(
                    f"{base}/files/stat",
                    json={"path": path, "chat_id": chat_id},
                )
                resp.raise_for_status()
                data = resp.json()
                if data.get("exists"):
                    return data
        except Exception:
            logger.debug("stat failed for sandbox path %s", path, exc_info=True)
        return None

    async def _fetch_binary(
        self,
        doc,
        document_name: str,
        context: ToolContext,
        *,
        deterministic_path: str | None = None,
    ) -> ToolResult:
        """Fetch binary file from source via connector-manager and write to sandbox.

        For deterministic (mention) paths, the caller provides a pre-computed
        path; for ordinary tool calls the filename from the response header
        or document_name is used.
        """
        logger.info(
            f"Fetching binary file '{document_name}' (id={doc.id}) from source {doc.source_id}"
        )

        async with httpx.AsyncClient(timeout=120.0) as client:
            resp = await client.post(
                f"{self._connector_manager_url}/action",
                json={
                    "source_id": doc.source_id,
                    "user_id": context.user_id,
                    "action": "fetch_file",
                    "params": {"document_id": doc.id},
                },
            )
            resp.raise_for_status()

            content_type = resp.headers.get("content-type", "")

            if "application/json" in content_type:
                result = resp.json()
                error = result.get("error", "Unknown error")
                return ToolResult(
                    content=[
                        {
                            "type": "text",
                            "text": f"Failed to fetch file: {error}",
                        }
                    ],
                    is_error=True,
                )

            binary_data = resp.content
            header_name = resp.headers.get("x-file-name")
            # Use the decoded header name for ordinary tool calls; for
            # deterministic mention paths use the caller-provided path.
            file_name = (
                deterministic_path
                if deterministic_path
                else (unquote(header_name) if header_name else document_name)
            )

        return await write_binary_to_sandbox(
            self._sandbox_url, binary_data, file_name, context.chat_id
        )

    async def _read_text(
        self,
        doc,
        document_name: str,
        start_line: int | None,
        end_line: int | None,
        context: ToolContext,
        *,
        deterministic_path: str | None = None,
    ) -> ToolResult:
        """Read text document content, returning directly or writing to sandbox."""
        if not doc.content_id:
            return ToolResult(
                content=[
                    {
                        "type": "text",
                        "text": f"Document '{document_name}' has no text content available.",
                    }
                ],
                is_error=True,
            )

        content = await self._content_storage.get_text(doc.content_id)

        if _is_pdf_extraction_failure(doc.content_type, content):
            if self._connector_manager_url and self._sandbox_url and doc.source_id:
                return await self._fetch_binary(
                    doc,
                    document_name,
                    context,
                    deterministic_path=deterministic_path,
                )
            return ToolResult(
                content=[
                    {
                        "type": "text",
                        "text": (
                            f"Document '{document_name}' could not be loaded: PDF text "
                            "extraction failed and binary staging is unavailable."
                        ),
                    }
                ],
                is_error=True,
            )

        if start_line is not None or end_line is not None:
            lines = content.split("\n")
            start = (start_line or 1) - 1
            end = end_line or len(lines)
            content = "\n".join(lines[start:end])

        if len(content) <= DIRECT_RETURN_THRESHOLD:
            return ToolResult(
                content=[
                    {
                        "type": "document",
                        "source": {
                            "type": "text",
                            "media_type": "text/plain",
                            "data": content,
                        },
                        "title": doc.title or document_name,
                        "citations": {"enabled": True},
                    }
                ],
            )

        if self._sandbox_url:
            filepath = deterministic_path
            if not filepath:
                filepath = document_name or doc.title or f"document_{doc.id}"
                if "." not in filepath:
                    filepath += ".txt"

            size_kb = len(content.encode("utf-8")) / 1024
            return await write_text_to_sandbox(
                self._sandbox_url,
                content,
                filepath,
                context.chat_id,
                message=f"Document saved to workspace: {filepath} ({size_kb:.1f} KB). Use read_file or run_python to process it.",
            )

        truncated = content[:DIRECT_RETURN_THRESHOLD]
        return ToolResult(
            content=[
                {
                    "type": "text",
                    "text": f"{truncated}\n\n... (truncated, {len(content)} total characters)",
                }
            ],
        )
