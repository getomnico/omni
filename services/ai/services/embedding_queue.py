"""Priority queue service for managing embedding requests."""

import asyncio
import logging
import time

from schemas import Priority, PrioritizedRequest, EmbeddingRequest, EmbeddingResponse
from state import AppState

logger = logging.getLogger(__name__)


class EmbeddingQueueService:
    """Service for managing prioritized embedding requests via an async queue."""

    def __init__(self, app_state: "AppState", maxsize: int = 100):
        self.app_state = app_state
        self._queue: asyncio.PriorityQueue = asyncio.PriorityQueue(maxsize=maxsize)
        self._processor_task: asyncio.Task | None = None

    async def start(self):
        """Start the queue processor task."""
        self._processor_task = asyncio.create_task(self._process_queue())
        logger.info("Embedding queue service started")

    async def stop(self):
        """Stop the queue processor task."""
        if self._processor_task:
            self._processor_task.cancel()
            try:
                await self._processor_task
            except asyncio.CancelledError:
                pass
        logger.info("Embedding queue service stopped")

    async def enqueue(
        self, request: EmbeddingRequest, request_id: str
    ) -> asyncio.Future:
        """Add a request to the queue and return a future for the result."""
        future: asyncio.Future = asyncio.Future()

        # Map priority string to enum
        priority_map = {
            "high": Priority.HIGH,
            "normal": Priority.NORMAL,
            "low": Priority.LOW,
        }
        priority = priority_map.get(request.priority, Priority.NORMAL)

        prioritized = PrioritizedRequest(
            priority=priority,
            request_id=request_id,
            request=request,
            future=future,
        )
        await self._queue.put(prioritized)

        # Log queue size if it's getting large
        queue_size = self._queue.qsize()
        if queue_size > 10:
            logger.warning(f"Embedding queue size: {queue_size}")

        return future

    @property
    def qsize(self) -> int:
        """Return the current queue size."""
        return self._queue.qsize()

    async def _process_queue(self):
        """Process embedding requests from the priority queue."""
        while True:
            try:
                # Get the highest priority request
                prioritized_request = await self._queue.get()
                request = prioritized_request.request
                future = prioritized_request.future

                # Log queue wait time for monitoring
                wait_time = time.time() - prioritized_request.timestamp
                if wait_time > 1.0:
                    logger.warning(
                        f"Request {prioritized_request.request_id} waited {wait_time:.2f}s in queue (priority: {prioritized_request.priority})"
                    )

                try:
                    # Process the embedding request using the provider
                    chunk_batch = (
                        await self.app_state.embedding_provider.generate_embeddings(
                            request.texts,
                            request.task,
                            request.chunk_size,
                            request.chunking_mode,
                        )
                    )

                    response = EmbeddingResponse(
                        embeddings=[
                            [c.embedding for c in chunks] for chunks in chunk_batch
                        ],
                        chunks_count=[len(chunks) for chunks in chunk_batch],
                        chunks=[[c.span for c in chunks] for chunks in chunk_batch],
                        model_name=self.app_state.embedding_provider.get_model_name(),
                    )

                    # Set the result on the future
                    future.set_result(response)

                except Exception as e:
                    logger.error(f"Failed to process embedding request: {e}")
                    future.set_exception(e)

            except asyncio.CancelledError:
                logger.info("Queue processor task cancelled")
                break
            except Exception as e:
                logger.error(f"Error in queue processor: {e}")
                await asyncio.sleep(0.1)  # Brief pause on error
