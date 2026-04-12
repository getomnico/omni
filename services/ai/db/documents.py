"""Repository for document-related database operations."""

import logging
from typing import Optional, List
from dataclasses import dataclass
from asyncpg import Pool

from .connection import get_db_pool

logger = logging.getLogger(__name__)

_COLUMNS = (
    "id, content_id, source_id, external_id, title, content_type, embedding_status, content_fingerprint"
)


def _permission_filter(user_email: str) -> str:
    return f"""
    AND (
        permissions @@@ 'public:true'
        OR permissions @@@ 'users:{user_email}'
        OR permissions @@@ 'groups:{user_email}'
    )
"""


@dataclass
class Document:
    """Document record from database"""

    id: str
    content_id: Optional[str]
    source_id: Optional[str] = None
    external_id: Optional[str] = None
    title: Optional[str] = None
    content_type: Optional[str] = None
    embedding_status: Optional[str] = None
    content_fingerprint: Optional[str] = None


@dataclass
class ContentBlob:
    """Content blob record from database"""

    id: str
    content_type: Optional[str]
    storage_key: str
    storage_backend: str


class DocumentsRepository:
    """Repository for document-related database operations."""

    def __init__(self, pool: Optional[Pool] = None):
        self.pool = pool

    async def _get_pool(self) -> Pool:
        """Get database pool"""
        if self.pool:
            return self.pool
        return await get_db_pool()

    async def get_by_id(
        self, document_id: str, user_email: str | None = None
    ) -> Optional[Document]:
        """Get a document by ID.

        When user_email is provided, the query enforces permission checks:
        the document is returned only if it is public, or the email appears
        in the document's users or groups list.  This mirrors the searcher's
        permission filter so the logic lives in one place (the DB query).
        """
        pool = await self._get_pool()

        if user_email:
            perm_filter = _permission_filter(user_email.lower())
            query = f"SELECT {_COLUMNS} FROM documents WHERE id = $1 {perm_filter}"
            row = await pool.fetchrow(query, document_id)
        else:
            query = f"SELECT {_COLUMNS} FROM documents WHERE id = $1"
            row = await pool.fetchrow(query, document_id)

        if row:
            return Document(
                id=row["id"],
                content_id=row["content_id"],
                source_id=row["source_id"],
                external_id=row["external_id"],
                title=row["title"],
                content_type=row["content_type"],
                embedding_status=row["embedding_status"],
                content_fingerprint=row["content_fingerprint"],
            )
        return None

    async def find_embedded_duplicate(
        self, content_fingerprint: str, exclude_document_id: str
    ) -> Optional[str]:
        """Find another document with the same content_fingerprint that already has embeddings.

        Returns the document ID if found, None otherwise.
        """
        pool = await self._get_pool()
        row = await pool.fetchrow(
            "SELECT id FROM documents WHERE content_fingerprint = $1 AND id != $2 AND embedding_status = 'completed' LIMIT 1",
            content_fingerprint,
            exclude_document_id,
        )
        return row["id"] if row else None

    async def get_content_blob(self, content_id: str) -> Optional[ContentBlob]:
        """Get content blob by ID"""
        pool = await self._get_pool()

        row = await pool.fetchrow(
            "SELECT id, content_type, storage_key, storage_backend FROM content_blobs WHERE id = $1",
            content_id,
        )

        if row:
            return ContentBlob(
                id=row["id"],
                content_type=row["content_type"],
                storage_key=row["storage_key"],
                storage_backend=row["storage_backend"],
            )
        return None

    async def update_embedding_status(
        self, document_ids: List[str], status: str
    ) -> None:
        """Update embedding_status for documents"""
        if not document_ids:
            return

        pool = await self._get_pool()

        await pool.execute(
            """
            UPDATE documents
            SET embedding_status = $2
            WHERE id = ANY($1)
            """,
            document_ids,
            status,
        )
        logger.info(
            f"Updated {len(document_ids)} documents to embedding_status: {status}"
        )
