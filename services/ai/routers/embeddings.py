"""Embeddings endpoint."""

import logging
import uuid

from fastapi import APIRouter, HTTPException, Request

from schemas import EmbeddingRequest, EmbeddingResponse

logger = logging.getLogger(__name__)
router = APIRouter(tags=["embeddings"])


@router.post("/embeddings", response_model=EmbeddingResponse)
async def generate_embeddings(request: Request, body: EmbeddingRequest):
    """Generate embeddings for input texts using configurable chunking.

    Chunking behavior:
    - fixed mode: chunk_size sets the number of tokens per chunk
    - sentence mode: groups sentences until chunk_size tokens limit
    - none mode: embed entire text without chunking
    """
    logger.info(
        f"Generating embeddings for {len(body.texts)} texts with priority={body.priority}, chunking_mode={body.chunking_mode}, chunk_size={body.chunk_size}"
    )

    # Validate chunking method
    valid_chunking_modes = ["sentence", "fixed", "none"]
    if body.chunking_mode not in valid_chunking_modes:
        raise HTTPException(
            status_code=422,
            detail=f"Invalid chunking_mode: {body.chunking_mode}. Must be one of: {valid_chunking_modes}",
        )

    try:
        # Generate a unique request ID
        request_id = str(uuid.uuid4())[:8]

        # Enqueue the request and wait for the result
        future = await request.app.state.embedding_queue.enqueue(body, request_id)
        response = await future

        logger.info(
            f"Generated these many chunks for each input text: {response.chunks_count}"
        )
        return response

    except Exception as e:
        logger.error(f"Failed to generate embeddings: {str(e)}")
        raise HTTPException(status_code=500, detail=str(e))
