"""Repository for embeddings table database operations."""

import logging
from dataclasses import dataclass
from datetime import datetime
from typing import Any, Dict, List, Optional

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

    async def bulk_clone_for_documents(
        self, document_pairs: list[tuple[str, str]], model_name: str
    ) -> dict[str, int]:
        """Clone current-model embeddings for many donor/target document pairs.

        document_pairs contains (source_document_id, target_document_id). Returns
        target_document_id -> cloned row count. Existing embeddings are replaced for targets
        that successfully clone rows.
        """
        if not document_pairs:
            return {}

        source_document_ids = [source_id for source_id, _ in document_pairs]
        target_document_ids = [target_id for _, target_id in document_pairs]
        pool = await self._get_pool()

        async with pool.acquire() as conn:
            async with conn.transaction():
                count_rows = await conn.fetch(
                    """
                    WITH raw_pairs AS (
                        SELECT source_document_id, target_document_id
                        FROM UNNEST($1::text[], $2::text[])
                            AS p(source_document_id, target_document_id)
                    ),
                    clone_pairs AS (
                        SELECT DISTINCT ON (target_document_id)
                               source_document_id, target_document_id
                        FROM raw_pairs
                        ORDER BY target_document_id, source_document_id
                    )
                    SELECT p.target_document_id AS document_id, count(*) AS cloned_count
                    FROM clone_pairs p
                    JOIN embeddings e
                      ON e.document_id = p.source_document_id
                     AND e.model_name = $3
                    GROUP BY p.target_document_id
                    """,
                    source_document_ids,
                    target_document_ids,
                    model_name,
                )
                clone_counts = {
                    row["document_id"]: int(row["cloned_count"]) for row in count_rows
                }
                if not clone_counts:
                    return {}

                await conn.execute(
                    """
                    DELETE FROM embeddings
                    WHERE document_id = ANY($1)
                    """,
                    list(clone_counts.keys()),
                )

                await conn.execute(
                    """
                    WITH raw_pairs AS (
                        SELECT source_document_id, target_document_id
                        FROM UNNEST($1::text[], $2::text[])
                            AS p(source_document_id, target_document_id)
                    ),
                    clone_pairs AS (
                        SELECT DISTINCT ON (target_document_id)
                               source_document_id, target_document_id
                        FROM raw_pairs
                        ORDER BY target_document_id, source_document_id
                    )
                    INSERT INTO embeddings (
                        id,
                        document_id,
                        chunk_index,
                        chunk_start_offset,
                        chunk_end_offset,
                        embedding,
                        model_name,
                        dimensions
                    )
                    SELECT
                        substring(
                            md5(
                                p.target_document_id || ':' || e.chunk_index || ':' || e.model_name
                                || ':' || e.chunk_start_offset || ':' || e.chunk_end_offset
                            ),
                            1,
                            26
                        ) AS id,
                        p.target_document_id,
                        e.chunk_index,
                        e.chunk_start_offset,
                        e.chunk_end_offset,
                        e.embedding,
                        e.model_name,
                        e.dimensions
                    FROM clone_pairs p
                    JOIN embeddings e
                      ON e.document_id = p.source_document_id
                     AND e.model_name = $3
                    """,
                    source_document_ids,
                    target_document_ids,
                    model_name,
                )

        logger.info(
            f"Cloned {sum(clone_counts.values())} embeddings for {len(clone_counts)} documents"
        )
        return clone_counts

    async def clone_for_document(
        self, source_document_id: str, target_document_id: str
    ) -> int:
        """Clone all embeddings from one document to another.

        Creates new embedding rows with fresh IDs pointing to target_document_id,
        copying all chunk data and vectors from source_document_id.

        Returns the number of cloned embeddings.
        """
        import ulid as _ulid

        existing = await self.get_for_document(source_document_id)
        if not existing:
            return 0

        cloned = [
            {
                "id": str(_ulid.ULID()),
                "document_id": target_document_id,
                "chunk_index": emb.chunk_index,
                "chunk_start_offset": emb.chunk_start_offset,
                "chunk_end_offset": emb.chunk_end_offset,
                "embedding": emb.embedding,
                "model_name": emb.model_name,
                "dimensions": emb.dimensions,
            }
            for emb in existing
        ]
        await self.bulk_insert(cloned)
        return len(cloned)
