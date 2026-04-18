"""Repository for document-related database operations."""

import logging
from typing import Optional, List
from dataclasses import dataclass
from asyncpg import Pool

from .connection import get_db_pool, user_db_connection

logger = logging.getLogger(__name__)

_COLUMNS = (
    "id, content_id, source_id, external_id, title, content_type, embedding_status"
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


@dataclass
class ContentBlob:
    """Content blob record from database"""

    id: str
    content_type: Optional[str]
    storage_key: str
    storage_backend: str


class DocumentsRepository:
    """Repository for document-related database operations."""

    def __init__(self, pool: Optional[Pool] = None, user_id: Optional[str] = None):
        self.pool = pool
        self.user_id = user_id

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
        if self.user_id:
            if user_email:
                perm_filter = _permission_filter(user_email.lower())
                query = f"SELECT {_COLUMNS} FROM documents WHERE id = $1 {perm_filter}"
                async with user_db_connection(self.user_id) as conn:
                    row = await conn.fetchrow(query, document_id)
            else:
                query = f"SELECT {_COLUMNS} FROM documents WHERE id = $1"
                async with user_db_connection(self.user_id) as conn:
                    row = await conn.fetchrow(query, document_id)
        else:
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
                source_id=row.get("source_id"),
                external_id=row.get("external_id"),
                title=row.get("title"),
                content_type=row.get("content_type"),
                embedding_status=row.get("embedding_status"),
            )
        return None

    async def get_content_blob(self, document_id: str) -> Optional[ContentBlob]:
        """Get the content blob for a document"""
        pool = await self._get_pool()
        query = """
            SELECT id, content_type, storage_key, storage_backend
            FROM documents
            WHERE id = $1
        """
        row = await pool.fetchrow(query, document_id)
        if row:
            return ContentBlob(
                id=row["id"],
                content_type=row.get("content_type"),
                storage_key=row["storage_key"],
                storage_backend=row["storage_backend"],
            )
        return None

    async def get_document_content(self, document_id: str) -> Optional[str]:
        """Get the content of a document"""
        pool = await self._get_pool()
        query = """
            SELECT content
            FROM documents
            WHERE id = $1
        """
        row = await pool.fetchrow(query, document_id)
        if row and row["content"]:
            return row["content"]
        return None
