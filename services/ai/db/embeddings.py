"""Repository for embeddings table database operations."""

import logging
from typing import List, Dict, Any, Optional
from dataclasses import dataclass
from datetime import datetime
from asyncpg import Pool

from .connection import get_db_pool

logger = logging.getLogger(__name__)


@dataclass
class Embedding:
    """Represents an embedding row."""

    id: str
    document_id: str
    chunk_index: int
    chunk_start_offset: int
    chunk_end_offset: int
    embedding: list
    model_name: str
    dimensions: int


class EmbeddingsRepository:
    """Repository for embeddings table database operations."""

    def __init__(self, pool: Optional[Pool] = None):
        self.pool = pool

    async def _get_pool(self) -> Pool:
        """Get database pool"""
        if self.pool:
            return self.pool
        return await get_db_pool()

    async def get_for_document(self, document_id: str) -> List[Embedding]:
        """Get all embeddings for a document, ordered by chunk_index."""
        pool = await self._get_pool()

        rows = await pool.fetch(
            """
            SELECT id, document_id, chunk_index, chunk_start_offset, chunk_end_offset,
                   embedding, model_name, dimensions
            FROM embeddings
            WHERE document_id = $1
            ORDER BY chunk_index
            """,
            document_id,
        )
        return [Embedding(**dict(row)) for row in rows]

    async def delete_for_documents(self, document_ids: List[str]) -> None:
        """Delete existing embeddings for documents"""
        if not document_ids:
            return

        pool = await self._get_pool()

        await pool.execute(
            """
            DELETE FROM embeddings
            WHERE document_id = ANY($1)
            """,
            document_ids,
        )
        logger.info(f"Deleted existing embeddings for {len(document_ids)} documents")

    async def bulk_insert(self, embeddings: List[Dict[str, Any]]) -> None:
        """Bulk insert embeddings into database using COPY for efficiency.

        Each embedding dict should contain:
        - id: str (ULID)
        - document_id: str
        - chunk_index: int
        - chunk_start_offset: int
        - chunk_end_offset: int
        - embedding: List[float]
        - model_name: str
        - dimensions: int
        - created_at: datetime (optional, defaults to now)
        """
        if not embeddings:
            return

        pool = await self._get_pool()

        # Prepare data for COPY
        records = [
            (
                emb["id"],
                emb["document_id"],
                emb["chunk_index"],
                emb["chunk_start_offset"],
                emb["chunk_end_offset"],
                emb["embedding"],
                emb["model_name"],
                emb["dimensions"],
                emb.get("created_at", datetime.utcnow()),
            )
            for emb in embeddings
        ]

        # Use COPY for efficient bulk insert
        await pool.copy_records_to_table(
            "embeddings",
            records=records,
            columns=[
                "id",
                "document_id",
                "chunk_index",
                "chunk_start_offset",
                "chunk_end_offset",
                "embedding",
                "model_name",
                "dimensions",
                "created_at",
            ],
        )
        logger.info(f"Bulk inserted {len(embeddings)} embeddings")
