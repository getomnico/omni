"""
Embedding processor for document indexing.

Drains the embedding_queue table by chunking each document and calling the
configured embedding provider.
"""

import asyncio
import logging
import time
from typing import Optional

import ulid

from config import EMBEDDING_MAX_MODEL_LEN

from . import Chunk
from db import (
    get_db_pool,
    DocumentsRepository,
    EmbeddingQueueRepository,
    EmbeddingsRepository,
    EmbeddingQueueItem,
    QueueStatus,
)
from state import AppState


logger = logging.getLogger(__name__)


# Configuration for online processing
ONLINE_BATCH_SIZE = 10
ONLINE_POLL_INTERVAL = 5  # Seconds to wait when queue is empty
ONLINE_BATCH_DELAY = 0.1  # Seconds to yield between batches when queue has items
PROGRESS_LOG_INTERVAL = 30  # Seconds between progress log lines
MAX_EMBEDDING_RETRIES = 5


class EmbeddingBatchProcessor:
    """Drains the embedding_queue table using the configured provider's online API."""

    def __init__(
        self,
        documents_repo: DocumentsRepository,
        queue_repo: EmbeddingQueueRepository,
        embeddings_repo: EmbeddingsRepository,
        app_state: AppState,
    ):
        self.documents_repo = documents_repo
        self.queue_repo = queue_repo
        self.embeddings_repo = embeddings_repo
        self.app_state = app_state

        self._embedding_semaphore = asyncio.Semaphore(1)

        # Progress tracking (populated at online loop start)
        self._progress_start_time: Optional[float] = None
        self._docs_completed = 0
        self._docs_failed = 0
        self._embeddings_written = 0
        self._embedding_time_ms: float = 0
        self._baseline_completed = 0
        self._baseline_failed = 0
        self._last_progress_log_time: Optional[float] = None

    @property
    def embedding_provider(self):
        return self.app_state.embedding_provider

    @property
    def provider_type(self) -> str:
        return self.app_state.embedding_provider_type or "local"

    @property
    def content_storage(self):
        return self.app_state.content_storage

    async def processing_loop(self):
        """Process queue items using online API calls"""
        logger.info(f"Starting embedding processor for provider: {self.provider_type}")

        status_counts = await self.queue_repo.get_status_counts()
        self._baseline_completed = status_counts.get(QueueStatus.COMPLETED, 0)
        self._baseline_failed = status_counts.get(QueueStatus.FAILED, 0)
        pending = status_counts.get(QueueStatus.PENDING, 0) + status_counts.get(
            QueueStatus.PROCESSING, 0
        )
        logger.info(
            f"Embedding queue: {pending} pending, "
            f"{self._baseline_completed} completed, "
            f"{self._baseline_failed} failed"
        )
        self._progress_start_time = time.time()
        self._last_progress_log_time = self._progress_start_time

        while True:
            try:
                processed_any = await self._process_online_batch()
                # Yield between batches - longer delay when actively processing
                # to allow higher-priority tasks (stream requests) to run
                if processed_any:
                    await asyncio.sleep(ONLINE_BATCH_DELAY)
            except Exception as e:
                logger.error(f"Online processing loop error: {e}", exc_info=True)
                await asyncio.sleep(10)

    async def _process_online_batch(self) -> bool:
        """Process a batch of queue items using online embedding API.

        Returns:
            True if any items were processed, False if queue was empty.
        """
        items = await self.queue_repo.get_pending_items(
            limit=ONLINE_BATCH_SIZE, max_retries=MAX_EMBEDDING_RETRIES
        )

        if not items:
            await asyncio.sleep(ONLINE_POLL_INTERVAL)
            return False

        logger.info(f"Processing {len(items)} documents via online embedding API")

        for item in items:
            try:
                await self._process_single_document(item)
            except Exception as e:
                logger.error(
                    f"Failed to process document {item.document_id}: {e}", exc_info=True
                )
                await self.queue_repo.mark_failed([item.id], str(e))
                self._docs_failed += 1
            finally:
                # Yield to allow higher-priority tasks (stream requests) to run
                await asyncio.sleep(0)
                await self._maybe_log_progress()

        return True

    async def _process_single_document(self, item: EmbeddingQueueItem):
        """Process a single document using the embedding provider"""
        if item.retry_count > 0:
            logger.debug(
                f"Retrying document {item.document_id} (attempt {item.retry_count + 1})"
            )

        # Use semaphore to limit concurrent embedding operations and yield more frequently
        async with self._embedding_semaphore:
            doc = await self.documents_repo.get_by_id(item.document_id)

            if not doc or not doc.content_id:
                logger.warning(
                    f"Document {item.document_id} has no content_id, skipping"
                )
                await self.queue_repo.mark_failed(
                    [item.id], "Document has no content_id"
                )
                self._docs_failed += 1
                return

            # Cross-source embedding cloning: if another document with the same
            # external_id already has embeddings, clone them instead of
            # regenerating.  This avoids duplicate vectors in the HNSW index
            # for IMAP threads ingested from multiple accounts.
            if doc.external_id and doc.external_id.startswith("imap-thread:"):
                donor_id = await self.documents_repo.find_embedded_duplicate(
                    doc.external_id, item.document_id
                )
                if donor_id:
                    await self.embeddings_repo.delete_for_documents([item.document_id])
                    cloned = await self.embeddings_repo.clone_for_document(
                        donor_id, item.document_id
                    )
                    if cloned > 0:
                        await self.queue_repo.mark_completed([item.id])
                        self._docs_completed += 1
                        self._embeddings_written += cloned
                        logger.info(
                            f"Cloned {cloned} embeddings from {donor_id} to {item.document_id} (cross-source dedup)"
                        )
                        return

            content_text = await self.content_storage.get_text(doc.content_id)

            if not content_text or not content_text.strip():
                logger.warning(
                    f"Document {item.document_id} has empty content, skipping"
                )
                await self.queue_repo.mark_failed(
                    [item.id], "Document has empty content"
                )
                self._docs_failed += 1
                return

            # Generate embeddings using sliding window over the document
            try:
                window_size = (
                    EMBEDDING_MAX_MODEL_LEN * 3
                )  # TODO: address 3 chars per token assumption here
                overlap = window_size // 4
                stride = window_size - overlap

                all_chunks = []
                offset = 0
                while offset < len(content_text):
                    piece = content_text[offset : offset + window_size]
                    t0 = time.monotonic()
                    chunk_results = await self.embedding_provider.generate_embeddings(
                        text=piece,
                        task="passage",
                        chunk_size=512,
                        chunking_mode="sentence",
                    )
                    elapsed_ms = (time.monotonic() - t0) * 1000
                    n_chunks = len(chunk_results) if chunk_results else 0
                    logger.debug(
                        f"generate_embeddings: {n_chunks} chunks in {elapsed_ms:.0f}ms "
                        f"({len(piece)} chars)"
                    )
                    self._embedding_time_ms += elapsed_ms

                    if chunk_results:
                        for chunk in chunk_results:
                            adjusted_span = (
                                offset + chunk.span[0],
                                offset + chunk.span[1],
                            )
                            all_chunks.append(Chunk(adjusted_span, chunk.embedding))

                    offset += stride

                chunks = all_chunks

                if not chunks:
                    logger.warning(
                        f"No embeddings generated for document {item.document_id}"
                    )
                    await self.queue_repo.mark_failed(
                        [item.id], "No embeddings generated"
                    )
                    self._docs_failed += 1
                    return

                await self.embeddings_repo.delete_for_documents([item.document_id])

                embeddings_to_insert = []
                for chunk_idx, chunk in enumerate(chunks):
                    embeddings_to_insert.append(
                        {
                            "id": str(ulid.ULID()),
                            "document_id": item.document_id,
                            "chunk_index": chunk_idx,
                            "chunk_start_offset": chunk.span[0],
                            "chunk_end_offset": chunk.span[1],
                            "embedding": chunk.embedding,
                            "model_name": self.embedding_provider.get_model_name(),
                            "dimensions": len(chunk.embedding),
                        }
                    )

                await self.embeddings_repo.bulk_insert(embeddings_to_insert)

                await self.queue_repo.mark_completed([item.id])

                self._docs_completed += 1
                self._embeddings_written += len(chunks)
                logger.info(
                    f"Processed document {item.document_id}: {len(chunks)} chunks embedded"
                )

            except Exception as e:
                logger.error(
                    f"Embedding generation failed for {item.document_id}: {e}",
                    exc_info=True,
                )
                await self.queue_repo.mark_failed([item.id], str(e))
                self._docs_failed += 1

    async def _maybe_log_progress(self):
        """Log embedding progress periodically."""
        if self._last_progress_log_time is None:
            return

        now = time.time()
        if now - self._last_progress_log_time < PROGRESS_LOG_INTERVAL:
            return

        self._last_progress_log_time = now
        pending = await self.queue_repo.get_pending_count(
            max_retries=MAX_EMBEDDING_RETRIES
        )
        total_completed = self._baseline_completed + self._docs_completed
        total_failed = self._baseline_failed + self._docs_failed

        elapsed_min = (now - self._progress_start_time) / 60
        docs_per_min = self._docs_completed / elapsed_min if elapsed_min > 0 else 0
        chunks_per_min = (
            self._embeddings_written / elapsed_min if elapsed_min > 0 else 0
        )

        eta = f"~{pending / docs_per_min:.1f} min" if docs_per_min > 0 else "unknown"
        avg_embed_ms = (
            self._embedding_time_ms / self._docs_completed
            if self._docs_completed > 0
            else 0
        )

        logger.info(
            f"Embedding progress: {pending} pending | "
            f"{total_completed} completed, {total_failed} failed | "
            f"Throughput: {docs_per_min:.1f} docs/min, {chunks_per_min:.0f} chunks/min | "
            f"Avg embed time: {avg_embed_ms:.0f}ms/doc | "
            f"ETA: {eta}"
        )


async def start_batch_processing(app_state: AppState):
    """Start batch processing background tasks.

    The processor reads the current embedding provider and provider type from
    app_state on each iteration, so hot-swapping providers doesn't require a restart.
    """
    if not app_state.embedding_provider:
        logger.warning("No embedding provider configured, skipping batch processing")
        return

    logger.info(
        f"Starting embedding batch processing with provider: {app_state.embedding_provider_type}"
    )

    db_pool = await get_db_pool()

    documents_repo = DocumentsRepository(db_pool)
    queue_repo = EmbeddingQueueRepository(db_pool)
    embeddings_repo = EmbeddingsRepository(db_pool)

    processor = EmbeddingBatchProcessor(
        documents_repo=documents_repo,
        queue_repo=queue_repo,
        embeddings_repo=embeddings_repo,
        app_state=app_state,
    )

    await processor.processing_loop()
