"""Omni AI Service - Entry Point"""

import asyncio
import logging
import os

import uvicorn
from fastapi import FastAPI

from config import (
    MEMORY_ENABLED,
    MEMORY_PROVIDER,
    PORT,
)
from logger import setup_logging
from memory.providers import build_memory_provider
from routers import (
    agents_router,
    chat_router,
    embeddings_router,
    health_router,
    internal_router,
    memory_router,
    model_providers_router,
    prompts_router,
    uploads_router,
    usage_router,
)
from services import (
    EmbeddingQueueService,
    initialize_providers,
    shutdown_providers,
    start_batch_processor,
)
from state import AppState
from telemetry import init_telemetry

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
app.include_router(model_providers_router)
app.include_router(agents_router)
app.include_router(uploads_router)
app.include_router(usage_router)
app.include_router(internal_router)
app.include_router(memory_router)


@app.on_event("startup")
async def startup_event():
    """Initialize services on startup."""
    try:
        app.state.embedding_queue = EmbeddingQueueService(app.state)
        await app.state.embedding_queue.start()
        await initialize_providers(app.state)
        await start_batch_processor(app.state)

        if os.getenv("AGENTS_ENABLED", "false").lower() == "true":
            from agents.scheduler import run_agent_scheduler

            asyncio.create_task(run_agent_scheduler(app.state))

        if MEMORY_ENABLED:
            try:
                app.state.memory_provider = await build_memory_provider(
                    MEMORY_PROVIDER, app.state
                )
                if app.state.memory_provider is not None:
                    logger.info(f"Memory provider initialized: {MEMORY_PROVIDER}")
                else:
                    logger.warning(f"Memory provider {MEMORY_PROVIDER!r} returned None")
            except ValueError:
                # Unknown provider name — fail fast at startup.
                raise
            except Exception as e:
                app.state.memory_provider = None
                logger.warning(f"Memory initialization failed: {e}")
        else:
            app.state.memory_provider = None
            logger.info("MEMORY_ENABLED=false — memory feature disabled")
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

    uvicorn.run(app, host="0.0.0.0", port=PORT)
