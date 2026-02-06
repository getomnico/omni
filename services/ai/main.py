"""Omni AI Service - Entry Point"""

import logging
import uvicorn

from fastapi import FastAPI

from logger import setup_logging
from telemetry import init_telemetry
from state import AppState
from services import (
    EmbeddingQueueService,
    initialize_providers,
    shutdown_providers,
    start_batch_processor,
)
from routers import chat_router, health_router, embeddings_router, prompts_router

from config import (
    PORT,
    EMBEDDING_PROVIDER,
    EMBEDDING_MODEL,
    EMBEDDING_DIMENSIONS,
    LLM_PROVIDER,
    LLM_MODEL,
    LLM_API_URL,
    AWS_REGION,
)

setup_logging()
logger = logging.getLogger(__name__)

app = FastAPI(title="Omni AI Service", version="0.1.0")

app.state = AppState()  # type: ignore[assignment]

init_telemetry(app, "omni-ai")

# Include routers
app.include_router(health_router)
app.include_router(embeddings_router)
app.include_router(prompts_router)
app.include_router(chat_router)


@app.on_event("startup")
async def startup_event():
    """Initialize services on startup."""
    try:
        app.state.embedding_queue = EmbeddingQueueService(app.state)
        await app.state.embedding_queue.start()
        await initialize_providers(app.state)
        await start_batch_processor(app.state)
    except Exception as e:
        logger.error(f"Failed to initialize services: {e}")
        raise e


@app.on_event("shutdown")
async def shutdown_event():
    """Cleanup on shutdown."""
    if hasattr(app.state, "embedding_queue"):
        await app.state.embedding_queue.stop()
    await shutdown_providers(app.state)


if __name__ == "__main__":
    logger.info(f"Starting AI service on port {PORT}")
    logger.info(f"Embedding provider: {EMBEDDING_PROVIDER}, model: {EMBEDDING_MODEL}")
    logger.info(f"Embedding dimensions: {EMBEDDING_DIMENSIONS}")
    logger.info(f"LLM provider: {LLM_PROVIDER}, model: {LLM_MODEL or 'default'}")

    if LLM_PROVIDER == "vllm":
        logger.info(f"vLLM URL: {LLM_API_URL}")
    elif LLM_PROVIDER == "bedrock":
        logger.info(f"AWS Region: {AWS_REGION if AWS_REGION else 'Auto-detected'}")

    uvicorn.run(app, host="0.0.0.0", port=PORT)
