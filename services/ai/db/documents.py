"""Repository for document-related database operations."""

import logging
from typing import Optional, List
from dataclasses import dataclass
from asyncpg import Pool

from .connection import get_db_pool

logger = logging.getLogger(__name__)


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

    def __init__(self, pool: Optional[Pool] = None):
        self.pool = pool

    async def _get_pool(self) -> Pool:
        """Get database pool"""
        if self.pool:
            return self.pool
        return await get_db_pool()

    async def get_by_id(self, document_id: str) -> Optional[Document]:
        """Get a document by ID"""
        pool = await self._get_pool()

        row = await pool.fetchrow(
            "SELECT id, content_id, source_id, external_id, title, content_type, embedding_status FROM documents WHERE id = $1",
            document_id,
        )

        if row:
            return Document(
                id=row["id"],
                content_id=row["content_id"],
                source_id=row["source_id"],
                external_id=row["external_id"],
                title=row["title"],
                content_type=row["content_type"],
                embedding_status=row["embedding_status"],
            )
        return None

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
