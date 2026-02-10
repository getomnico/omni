"""
Main embeddings processor for document indexing.
Handles batch processing of documents from embedding_queue for all providers.

For Bedrock: Uses cloud batch inference (S3/JSONL workflow)
For other providers (local, openai, jina): Uses online API calls in batches
"""

import asyncio
import logging
import json
import time
from typing import List, Dict, Optional, Tuple, Any, AsyncGenerator
from collections import defaultdict
from datetime import datetime
import ulid

from config import (
    DATABASE_URL,
    EMBEDDING_BATCH_S3_BUCKET,
    EMBEDDING_BATCH_BEDROCK_ROLE_ARN,
    EMBEDDING_BATCH_MIN_DOCUMENTS,
    EMBEDDING_BATCH_MAX_DOCUMENTS,
    EMBEDDING_BATCH_ACCUMULATION_TIMEOUT_SECONDS,
    EMBEDDING_BATCH_ACCUMULATION_POLL_INTERVAL,
    EMBEDDING_BATCH_MONITOR_POLL_INTERVAL,
    EMBEDDING_MODEL,
    EMBEDDING_MAX_MODEL_LEN,
    AWS_REGION,
)

from processing.chunking import Chunker
from . import Chunk
from db import (
    get_db_pool,
    DocumentsRepository,
    EmbeddingQueueRepository,
    EmbeddingsRepository,
    EmbeddingBatchJobsRepository,
    EmbeddingQueueItem,
    BatchJob,
    QueueStatus,
)

# Import boto3 and smart_open lazily for Bedrock provider
boto3 = None
smart_open = None


def _import_bedrock_deps():
    """Import boto3 and smart_open lazily (only needed for Bedrock)"""
    global boto3, smart_open
    if boto3 is None:
        import boto3 as _boto3
        import smart_open as _smart_open

        boto3 = _boto3
        smart_open = _smart_open


logger = logging.getLogger(__name__)


# Configuration for online processing
ONLINE_BATCH_SIZE = 10
ONLINE_POLL_INTERVAL = 5  # Seconds to wait when queue is empty
ONLINE_BATCH_DELAY = 0.1  # Seconds to yield between batches when queue has items
PROGRESS_LOG_INTERVAL = 30  # Seconds between progress log lines
MAX_EMBEDDING_RETRIES = 5


# ============================================================================
# Storage Operations (Cloud-Agnostic Interface)
# ============================================================================
class StorageClient:
    """Abstraction for cloud storage operations (S3, GCS, etc.)"""

    async def upload_jsonl(self, path: str, records: AsyncGenerator[dict, None]) -> int:
        """Upload JSONL to cloud storage"""
        raise NotImplementedError

    async def download_jsonl(self, path: str) -> List[dict]:
        """Download JSONL from cloud storage"""
        raise NotImplementedError

    async def list_files(self, prefix: str) -> List[str]:
        """List files with given prefix"""
        raise NotImplementedError


class S3StorageClient(StorageClient):
    """S3-specific implementation"""

    def __init__(self, bucket: str, region: Optional[str] = None):
        _import_bedrock_deps()
        self.bucket = bucket
        self.region = region
        if region:
            self.s3_client = boto3.client("s3", region_name=region)
        else:
            self.s3_client = boto3.client("s3")
        logger.info(f"Initialized S3 storage client for bucket: {bucket}")

        self.max_records_per_file = 1000

    async def upload_jsonl(self, path: str, records: AsyncGenerator[dict, None]) -> int:
        """Upload JSONL to S3."""
        num_records = 0
        with smart_open.open(
            f"s3://{self.bucket}/{path}",
            "w",
            transport_params={"min_part_size": 25 * 1024 * 1024},
            buffering=5 * 1024 * 1024,
        ) as f:
            async for record in records:
                jsonl = json.dumps(record) + "\n"
                f.write(jsonl)
                num_records += 1

                if num_records % 100 == 0:
                    logger.info(
                        f"Uploaded {num_records} records to s3://{self.bucket}/{path}"
                    )

        logger.info(f"Uploaded {num_records} records to s3://{self.bucket}/{path}")
        return num_records

    async def download_jsonl(self, path: str) -> List[dict]:
        """Download JSONL from S3"""
        loop = asyncio.get_event_loop()
        response = await loop.run_in_executor(
            None, lambda: self.s3_client.get_object(Bucket=self.bucket, Key=path)
        )

        content = response["Body"].read().decode("utf-8")
        records = [
            json.loads(line) for line in content.strip().split("\n") if line.strip()
        ]
        logger.info(f"Downloaded {len(records)} records from s3://{self.bucket}/{path}")
        return records

    async def list_files(self, prefix: str) -> List[str]:
        """List files with given prefix in S3"""
        loop = asyncio.get_event_loop()
        response = await loop.run_in_executor(
            None,
            lambda: self.s3_client.list_objects_v2(Bucket=self.bucket, Prefix=prefix),
        )

        files = []
        if "Contents" in response:
            files = [obj["Key"] for obj in response["Contents"]]

        logger.debug(f"Found {len(files)} files with prefix: {prefix}")
        return files


# ============================================================================
# Provider Interface (Cloud-Agnostic)
# ============================================================================
class BatchInferenceProvider:
    """Abstract interface for batch inference providers"""

    async def submit_job(self, input_path: str, output_path: str, job_name: str) -> str:
        """Submit batch job, return external_job_id"""
        raise NotImplementedError

    async def get_job_status(self, external_job_id: str) -> Tuple[str, Optional[str]]:
        """Get job status, return (status, error_message)"""
        raise NotImplementedError


class BedrockBatchProvider(BatchInferenceProvider):
    """AWS Bedrock batch inference implementation"""

    def __init__(self, model_id: str, role_arn: str, region: Optional[str] = None):
        _import_bedrock_deps()
        self.model_id = model_id
        self.role_arn = role_arn
        if region:
            self.bedrock_client = boto3.client("bedrock", region_name=region)
        else:
            self.bedrock_client = boto3.client("bedrock")
        logger.info(f"Initialized Bedrock batch provider with model: {model_id}")

    async def submit_job(self, input_path: str, output_path: str, job_name: str) -> str:
        """Submit batch job to Bedrock"""
        loop = asyncio.get_event_loop()

        response = await loop.run_in_executor(
            None,
            lambda: self.bedrock_client.create_model_invocation_job(
                roleArn=self.role_arn,
                modelId=self.model_id,
                jobName=job_name,
                inputDataConfig={"s3InputDataConfig": {"s3Uri": input_path}},
                outputDataConfig={"s3OutputDataConfig": {"s3Uri": output_path}},
            ),
        )

        job_arn = response["jobArn"]
        logger.info(f"Submitted Bedrock batch job: {job_arn}")
        return job_arn

    async def get_job_status(self, external_job_id: str) -> Tuple[str, Optional[str]]:
        """Get job status from Bedrock"""
        loop = asyncio.get_event_loop()

        try:
            response = await loop.run_in_executor(
                None,
                lambda: self.bedrock_client.get_model_invocation_job(
                    jobIdentifier=external_job_id
                ),
            )

            status = response["status"]
            error_message = response.get("message") if status == "Failed" else None

            # Map Bedrock status to our internal status
            status_map = {
                "Submitted": "submitted",
                "InProgress": "processing",
                "Completed": "completed",
                "Failed": "failed",
                "Stopping": "processing",
                "Stopped": "failed",
            }

            internal_status = status_map.get(status, "processing")
            return (internal_status, error_message)

        except Exception as e:
            logger.error(f"Error getting job status for {external_job_id}: {e}")
            return ("failed", str(e))


# ============================================================================
# Batch Processing Logic
# ============================================================================
class EmbeddingBatchProcessor:
    """Main class that orchestrates batch processing for all embedding providers.

    For Bedrock: Uses cloud batch inference (S3/JSONL workflow)
    For other providers (local, openai, jina): Uses online API calls in batches
    """

    def __init__(
        self,
        documents_repo: DocumentsRepository,
        queue_repo: EmbeddingQueueRepository,
        embeddings_repo: EmbeddingsRepository,
        batch_jobs_repo: EmbeddingBatchJobsRepository,
        content_storage,
        embedding_provider,
        provider_type: str,
    ):
        self.documents_repo = documents_repo
        self.queue_repo = queue_repo
        self.embeddings_repo = embeddings_repo
        self.batch_jobs_repo = batch_jobs_repo
        self.content_storage = content_storage
        self.embedding_provider = embedding_provider
        self.provider_type = provider_type

        # Only initialize Bedrock-specific components when needed
        self.storage_client: Optional[StorageClient] = None
        self.batch_provider: Optional[BatchInferenceProvider] = None

        if provider_type == "bedrock":
            self.storage_client = self._create_storage_client()
            self.batch_provider = self._create_batch_provider()

        self.accumulation_state = {
            "last_seen_count": 0,
            "last_change_time": time.time(),
        }

        # Limit concurrent embedding operations to yield more frequently to higher-priority tasks
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

    def _create_storage_client(self) -> StorageClient:
        """Factory for storage client (Bedrock only)"""
        if EMBEDDING_BATCH_S3_BUCKET:
            return S3StorageClient(EMBEDDING_BATCH_S3_BUCKET, AWS_REGION)
        raise ValueError(
            "EMBEDDING_BATCH_S3_BUCKET must be configured for Bedrock provider"
        )

    def _create_batch_provider(self) -> BatchInferenceProvider:
        """Factory for batch provider (Bedrock only)"""
        if not EMBEDDING_MODEL or not EMBEDDING_BATCH_BEDROCK_ROLE_ARN:
            raise ValueError(
                "EMBEDDING_MODEL and EMBEDDING_BATCH_BEDROCK_ROLE_ARN must be configured for Bedrock provider"
            )

        return BedrockBatchProvider(
            EMBEDDING_MODEL, EMBEDDING_BATCH_BEDROCK_ROLE_ARN, AWS_REGION
        )

    # ------------------------------------------------------------------------
    # Main Processing Loop (routes based on provider type)
    # ------------------------------------------------------------------------
    async def processing_loop(self):
        """Main processing loop - routes to appropriate handler based on provider"""
        logger.info(f"Starting embedding processor for provider: {self.provider_type}")

        if self.provider_type == "bedrock":
            # Use cloud batch inference for Bedrock
            await asyncio.gather(self.accumulation_loop(), self.monitoring_loop())
        else:
            # Use online processing for other providers (local, openai, jina)
            await self.online_processing_loop()

    # ------------------------------------------------------------------------
    # Online Processing Loop (for local, openai, jina providers)
    # ------------------------------------------------------------------------
    async def online_processing_loop(self):
        """Process queue items using online API calls"""
        logger.info("Starting online embedding processing loop")

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
        # Get pending items
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
            # Fetch document
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

                # Handle empty chunks
                if not chunks:
                    logger.warning(
                        f"No embeddings generated for document {item.document_id}"
                    )
                    await self.queue_repo.mark_failed(
                        [item.id], "No embeddings generated"
                    )
                    self._docs_failed += 1
                    return

                # Delete existing embeddings for this document
                await self.embeddings_repo.delete_for_documents([item.document_id])

                # Prepare embeddings for bulk insert
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

                # Bulk insert embeddings
                await self.embeddings_repo.bulk_insert(embeddings_to_insert)

                # Mark queue item completed
                await self.queue_repo.mark_completed([item.id])

                # Update document embedding status
                await self.documents_repo.update_embedding_status(
                    [item.document_id], "completed"
                )

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

    # ------------------------------------------------------------------------
    # Accumulation Loop (Bedrock only)
    # ------------------------------------------------------------------------
    async def accumulation_loop(self):
        """Background task: accumulate items and trigger batch creation"""
        logger.info("Starting embedding batch accumulation loop")

        while True:
            try:
                await self._check_and_create_batch()
                await asyncio.sleep(EMBEDDING_BATCH_ACCUMULATION_POLL_INTERVAL)
            except Exception as e:
                logger.error(f"Accumulation loop error: {e}", exc_info=True)
                await asyncio.sleep(10)

    async def _check_and_create_batch(self):
        """Check thresholds and create batch if conditions met"""
        current_count = await self.queue_repo.get_pending_count(
            max_retries=MAX_EMBEDDING_RETRIES
        )
        logger.debug(f"Current pending items: {current_count}")
        if current_count == 0:
            return

        # Update accumulation state
        current_time = time.time()
        if current_count != self.accumulation_state["last_seen_count"]:
            self.accumulation_state["last_seen_count"] = current_count
            self.accumulation_state["last_change_time"] = current_time

        time_since_last_change = (
            current_time - self.accumulation_state["last_change_time"]
        )

        # Check thresholds
        should_create_batch = current_count >= EMBEDDING_BATCH_MIN_DOCUMENTS and (
            current_count >= EMBEDDING_BATCH_MAX_DOCUMENTS
            or time_since_last_change >= EMBEDDING_BATCH_ACCUMULATION_TIMEOUT_SECONDS
        )

        if should_create_batch:
            logger.info(
                f"Creating batch with {current_count} documents "
                f"(idle for {time_since_last_change:.1f}s)"
            )
            await asyncio.create_task(self._create_bedrock_batch())

            # Reset accumulation state
            self.accumulation_state["last_seen_count"] = 0
            self.accumulation_state["last_change_time"] = current_time
        else:
            logger.debug(
                f"Not creating batch yet: {current_count} documents "
                f"(idle for {time_since_last_change:.1f}s)"
            )

    async def _create_bedrock_batch(self):
        """Create and submit a new Bedrock batch job"""
        try:
            batch_id = await self.batch_jobs_repo.create("bedrock")
        except Exception as e:
            logger.error(f"Failed to create batch: {e}", exc_info=True)
            return

        try:
            await self.batch_jobs_repo.update_status(batch_id, status="preparing")

            async def record_generator():
                num_records_in_batch = 0
                items = await self.queue_repo.get_pending_items(
                    EMBEDDING_BATCH_MAX_DOCUMENTS, max_retries=MAX_EMBEDDING_RETRIES
                )
                for item in items:
                    jsonl_records = await self._fetch_and_chunk_document_for_bedrock(
                        item
                    )
                    if (
                        num_records_in_batch + len(jsonl_records)
                        >= EMBEDDING_BATCH_MAX_DOCUMENTS
                    ):
                        logger.info(
                            f"Batch {batch_id} reached max documents ({num_records_in_batch})"
                        )
                        break

                    logger.debug(
                        f"Including {len(jsonl_records)} records from document {item.document_id} in batch {batch_id}"
                    )
                    await self.queue_repo.assign_to_batch(batch_id, [item.id])
                    for record in jsonl_records:
                        yield record
                    num_records_in_batch += len(jsonl_records)

                await self.batch_jobs_repo.update_status(
                    batch_id, status="preparing", document_count=num_records_in_batch
                )

            # Upload to storage
            logger.info(f"Uploading batch {batch_id} to storage...")
            date = datetime.now().strftime("%Y-%m-%d")
            input_key = f"{date}/{batch_id}/input.jsonl"
            num_records_uploaded = await self.storage_client.upload_jsonl(
                input_key, record_generator()
            )
            logger.info(
                f"Completed upload of {num_records_uploaded} to s3://{EMBEDDING_BATCH_S3_BUCKET}/{input_key}"
            )

            # Submit job
            input_path = f"s3://{EMBEDDING_BATCH_S3_BUCKET}/{input_key}"
            output_path = f"s3://{EMBEDDING_BATCH_S3_BUCKET}/{batch_id}/output"
            job_name = f"embedding-batch-{batch_id}"
            logger.info(f"Submitting batch job {batch_id} to provider...")
            external_job_id = await self.batch_provider.submit_job(
                input_path, output_path, job_name
            )
            logger.info(
                f"Submitted batch {batch_id} to provider, external_job_id: {external_job_id}"
            )

            # Update batch job with submission details
            await self.batch_jobs_repo.update_status(
                batch_id,
                status="submitted",
                external_job_id=external_job_id,
                input_storage_path=input_path,
                output_storage_path=output_path,
                document_count=num_records_uploaded,
            )

            # Mark queue items as processing
            await self.queue_repo.mark_processing(batch_id)

            logger.info(
                f"Batch {batch_id} submitted successfully (external_job_id: {external_job_id})"
            )
        except Exception as e:
            logger.error(f"Failed to prepare batch {batch_id}: {e}")
            await self.batch_jobs_repo.update_status(
                batch_id, status="failed", error_message=str(e)
            )
            # Reset all items included in this batch
            items = await self.queue_repo.get_items_for_batch(batch_id)
            await self.queue_repo.mark_pending([item.id for item in items])

    async def _fetch_and_chunk_document_for_bedrock(
        self, item: EmbeddingQueueItem
    ) -> List[dict]:
        """Fetch a single document and generate JSONL records for Bedrock."""
        chunks = []

        try:
            doc = await self.documents_repo.get_by_id(item.document_id)

            if not doc or not doc.content_id:
                logger.error(f"Document {item.document_id} has no content_id, skipping")
                return chunks

            content_text = await self.content_storage.get_text(doc.content_id)
            if not content_text or not content_text.strip():
                logger.error(f"Empty content for document {item.document_id}")
                return chunks

            # Chunk the content
            chunk_spans = Chunker.chunk_sentences_by_chars(content_text, max_chars=4096)

            for chunk_idx, (start_char, end_char) in enumerate(chunk_spans):
                chunk_text = content_text[start_char:end_char]

                chunks.append(
                    {
                        "recordId": f"{item.document_id}:{chunk_idx}:{start_char}:{end_char}",
                        "modelInput": {"inputText": chunk_text},
                    }
                )

            logger.debug(
                f"Document {item.document_id} chunked into {len(chunks)} chunks."
            )

        except Exception as e:
            logger.error(
                f"Failed to process document {item.document_id}: {e}", exc_info=True
            )

        return chunks

    # ------------------------------------------------------------------------
    # Monitoring Loop (Bedrock only)
    # ------------------------------------------------------------------------
    async def monitoring_loop(self):
        """Background task: poll job status and process results"""
        logger.info("Starting embedding batch monitoring loop")

        while True:
            try:
                await self._monitor_active_jobs()
                await asyncio.sleep(EMBEDDING_BATCH_MONITOR_POLL_INTERVAL)
            except Exception as e:
                logger.error(f"Monitoring loop error: {e}", exc_info=True)
                await asyncio.sleep(30)

    async def _monitor_active_jobs(self):
        """Check status of all active jobs"""
        jobs = await self.batch_jobs_repo.get_active()

        if not jobs:
            return

        logger.debug(f"Monitoring {len(jobs)} active batch jobs")

        for job in jobs:
            try:
                status, error_message = await self.batch_provider.get_job_status(
                    job.external_job_id
                )

                if status == "completed":
                    logger.info(f"Batch job {job.id} completed, processing results")
                    await self._process_bedrock_results(job.id)

                elif status == "failed":
                    logger.error(f"Batch job {job.id} failed: {error_message}")
                    await self._mark_batch_failed(job.id, error_message)

                elif status != job.status:
                    await self.batch_jobs_repo.update_status(job.id, status)

            except Exception as e:
                logger.error(f"Error monitoring job {job.id}: {e}", exc_info=True)

    async def _mark_batch_failed(self, batch_id: str, error_message: Optional[str]):
        """Mark batch job and all its items as failed"""
        await self.batch_jobs_repo.update_status(
            batch_id, "failed", error_message=error_message
        )

        items = await self.queue_repo.get_items_for_batch(batch_id)
        await self.queue_repo.mark_failed(
            [item.id for item in items], error_message or "Batch job failed"
        )

    # ------------------------------------------------------------------------
    # Result Processing (Bedrock only)
    # ------------------------------------------------------------------------
    async def _process_bedrock_results(self, batch_id: str):
        """Download and process Bedrock batch results"""
        logger.info(f"Processing results for batch {batch_id}")

        try:
            job = await self.batch_jobs_repo.get_by_id(batch_id)

            if not job or not job.output_storage_path:
                raise ValueError(f"No output path for batch {batch_id}")

            # Extract prefix from output path
            output_path = job.output_storage_path
            prefix = output_path.replace(f"s3://{EMBEDDING_BATCH_S3_BUCKET}/", "")

            # List output files
            output_files = await self.storage_client.list_files(prefix)

            if not output_files:
                raise ValueError(f"No output files found for batch {batch_id}")

            # Download and parse all output files
            all_output_lines = []
            for file_key in output_files:
                if file_key.endswith(".jsonl") or file_key.endswith(".out"):
                    lines = await self.storage_client.download_jsonl(file_key)
                    all_output_lines.extend(lines)

            logger.info(
                f"Downloaded {len(all_output_lines)} output records for batch {batch_id}"
            )

            # Parse and group embeddings by document
            embeddings_by_doc = self._parse_bedrock_output(all_output_lines)

            # Store embeddings in database
            await self._store_bedrock_embeddings(embeddings_by_doc)

            # Mark queue items as completed
            items = await self.queue_repo.get_items_for_batch(batch_id)
            await self.queue_repo.mark_completed([item.id for item in items])

            # Update documents embedding_status
            document_ids = list(embeddings_by_doc.keys())
            await self.documents_repo.update_embedding_status(document_ids, "completed")

            # Mark batch job as completed
            await self.batch_jobs_repo.update_status(batch_id, "completed")

            logger.info(
                f"Successfully processed batch {batch_id} with {len(document_ids)} documents"
            )

        except Exception as e:
            logger.error(
                f"Failed to process results for batch {batch_id}: {e}", exc_info=True
            )
            await self._mark_batch_failed(batch_id, str(e))

    def _parse_bedrock_output(self, output_lines: List[dict]) -> Dict[str, List]:
        """Parse Bedrock JSONL output and group by document_id"""
        embeddings_by_doc = defaultdict(list)

        for line in output_lines:
            try:
                record_id = line["recordId"]

                if "error" in line:
                    logger.warning(f"Embedding error for {record_id}: {line['error']}")
                    continue

                model_output = line.get("modelOutput", {})
                embedding = model_output.get("embedding")

                if not embedding:
                    logger.warning(f"No embedding in output for {record_id}")
                    continue

                parts = record_id.split(":")
                if len(parts) != 4:
                    logger.warning(f"Invalid record_id format: {record_id}")
                    continue

                doc_id, chunk_idx, start, end = parts

                embeddings_by_doc[doc_id].append(
                    {
                        "chunk_index": int(chunk_idx),
                        "start": int(start),
                        "end": int(end),
                        "embedding": embedding,
                    }
                )

            except Exception as e:
                logger.error(f"Error parsing output line: {e}", exc_info=True)
                continue

        # Sort chunks by index for each document
        for doc_id in embeddings_by_doc:
            embeddings_by_doc[doc_id].sort(key=lambda x: x["chunk_index"])

        logger.info(f"Grouped embeddings for {len(embeddings_by_doc)} documents")
        return embeddings_by_doc

    async def _store_bedrock_embeddings(self, embeddings_by_doc: Dict[str, List]):
        """Bulk insert Bedrock embeddings into database"""
        if not embeddings_by_doc:
            return

        # Delete existing embeddings for these documents
        document_ids = list(embeddings_by_doc.keys())
        await self.embeddings_repo.delete_for_documents(document_ids)

        # Prepare embeddings for bulk insert
        all_embeddings = []
        for doc_id, chunks in embeddings_by_doc.items():
            for chunk in chunks:
                all_embeddings.append(
                    {
                        "id": str(ulid.ULID()),
                        "document_id": doc_id,
                        "chunk_index": chunk["chunk_index"],
                        "chunk_start_offset": chunk["start"],
                        "chunk_end_offset": chunk["end"],
                        "embedding": chunk["embedding"],
                        "model_name": EMBEDDING_MODEL,
                        "dimensions": len(chunk["embedding"]),
                    }
                )

        await self.embeddings_repo.bulk_insert(all_embeddings)

        logger.info(
            f"Stored {len(all_embeddings)} embeddings for {len(document_ids)} documents"
        )


# ============================================================================
# Public API for Integration
# ============================================================================
async def start_batch_processing(
    content_storage, embedding_provider, provider_type: str
):
    """Start batch processing background tasks.

    Args:
        content_storage: Storage client for fetching document content
        embedding_provider: The configured embedding provider
        provider_type: Type of provider ("bedrock", "local", "openai", "jina")
    """
    logger.info(f"Starting embedding batch processing with provider: {provider_type}")

    # Get shared database pool
    db_pool = await get_db_pool()

    # Create repository instances
    documents_repo = DocumentsRepository(db_pool)
    queue_repo = EmbeddingQueueRepository(db_pool)
    embeddings_repo = EmbeddingsRepository(db_pool)
    batch_jobs_repo = EmbeddingBatchJobsRepository(db_pool)

    processor = EmbeddingBatchProcessor(
        documents_repo=documents_repo,
        queue_repo=queue_repo,
        embeddings_repo=embeddings_repo,
        batch_jobs_repo=batch_jobs_repo,
        content_storage=content_storage,
        embedding_provider=embedding_provider,
        provider_type=provider_type,
    )

    # Start the processing loop
    await processor.processing_loop()
