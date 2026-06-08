"""Repository for document-related database operations."""

import logging
from dataclasses import dataclass
from typing import Optional

from asyncpg import Pool

from .connection import get_db_pool

logger = logging.getLogger(__name__)

_COLUMNS = "id, content_id, source_id, external_id, title, content_type"


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

    async def get_by_ids(self, document_ids: list[str]) -> dict[str, Document]:
        """Get documents by ID, keyed by document ID."""
        if not document_ids:
            return {}

        pool = await self._get_pool()
        rows = await pool.fetch(
            f"SELECT {_COLUMNS} FROM documents WHERE id = ANY($1)",
            document_ids,
        )

        return {
            row["id"]: Document(
                id=row["id"],
                content_id=row["content_id"],
                source_id=row["source_id"],
                external_id=row["external_id"],
                title=row["title"],
                content_type=row["content_type"],
            )
            for row in rows
        }

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
            )
        return None

    async def get_by_external_id(
        self, external_id: str, user_email: str | None = None
    ) -> Optional[Document]:
        """Get a document by its connector-native external_id.

        Mirrors get_by_id's permission filter. If multiple documents share the
        external_id (cross-source duplicates), returns the first match —
        acceptable for composite ids like Gmail's `{thread}:att:{msg}:{att}`
        which are practically unique.
        """
        pool = await self._get_pool()

        if user_email:
            perm_filter = _permission_filter(user_email.lower())
            query = f"SELECT {_COLUMNS} FROM documents WHERE external_id = $1 {perm_filter} LIMIT 1"
        else:
            query = f"SELECT {_COLUMNS} FROM documents WHERE external_id = $1 LIMIT 1"
        row = await pool.fetchrow(query, external_id)

        if row:
            return Document(
                id=row["id"],
                content_id=row["content_id"],
                source_id=row["source_id"],
                external_id=row["external_id"],
                title=row["title"],
                content_type=row["content_type"],
            )
        return None

    async def find_embedded_content_donors(
        self,
        content_ids: list[str],
        exclude_document_ids: list[str],
        model_name: str,
    ) -> dict[str, str]:
        """Find embedded donor documents for content IDs.

        Returns content_id -> donor_document_id for donors that already have embeddings for
        the current model. Donors with pending/processing work, or failed work newer than
        their embeddings, are excluded because their embeddings may be stale relative to
        their current content_id.
        """
        if not content_ids:
            return {}

        pool = await self._get_pool()
        rows = await pool.fetch(
            """
            WITH embedded_documents AS (
                SELECT document_id, max(created_at) AS latest_embedding_at
                FROM embeddings
                WHERE model_name = $3
                GROUP BY document_id
            )
            SELECT DISTINCT ON (d.content_id) d.content_id, d.id
            FROM documents d
            JOIN embedded_documents ed ON ed.document_id = d.id
            WHERE d.content_id = ANY($1)
              AND d.id <> ALL($2::text[])
              AND NOT EXISTS (
                  SELECT 1
                  FROM embedding_queue q
                  WHERE q.document_id = d.id
                    AND q.status IN ('pending', 'processing')
              )
              AND NOT EXISTS (
                  SELECT 1
                  FROM embedding_queue q
                  WHERE q.document_id = d.id
                    AND q.status = 'failed'
                    AND GREATEST(q.created_at, q.updated_at, COALESCE(q.processed_at, q.updated_at))
                        > ed.latest_embedding_at
              )
            ORDER BY d.content_id, d.id
            """,
            content_ids,
            exclude_document_ids,
            model_name,
        )
        return {row["content_id"]: row["id"] for row in rows}

    async def find_embedded_duplicate(
        self, external_id: str, exclude_document_id: str
    ) -> Optional[str]:
        """Find another document with the same external_id that already has embeddings.

        Used for cross-source dedup: IMAP threads from different accounts share
        the same external_id.  Instead of regenerating embeddings for a duplicate,
        we clone from the already-embedded document.

        Returns the donor document's ID if found, None otherwise.
        """
        pool = await self._get_pool()
        row = await pool.fetchrow(
            """
            SELECT d.id
            FROM documents d
            WHERE d.external_id = $1
              AND d.id != $2
              AND EXISTS (SELECT 1 FROM embeddings e WHERE e.document_id = d.id)
            LIMIT 1
            """,
            external_id,
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
