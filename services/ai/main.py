"""Omni AI Service - Entry Point"""

import asyncio
import logging
import os

import uvicorn
from fastapi import FastAPI
from fastapi.concurrency import run_in_threadpool
from mem0 import Memory

from config import (
    DATABASE_URL,
    MEM0_HISTORY_DB_PATH,
    MEM0AI_DATABASE_ROLE_PASSWORD,
    MEM0AI_DATABASE_USER,
    MEMORY_ENABLED,
    PORT,
)
from logger import setup_logging
from memory.bootstrap import MemoryConfigError, build_mem0_config
from memory.role_bootstrap import ensure_mem0ai_role
from memory.service import MemoryService
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
                ensure_mem0ai_role(
                    dsn=DATABASE_URL,
                    database_name=os.environ["DATABASE_NAME"],
                    database_username=os.environ["DATABASE_USERNAME"],
                    mem0ai_password=MEM0AI_DATABASE_ROLE_PASSWORD,
                    role_name=MEM0AI_DATABASE_USER,
                )
                cfg = await build_mem0_config(
                    app.state,
                    database_host=os.environ["DATABASE_HOST"],
                    database_port=int(os.environ.get("DATABASE_PORT", "5432")),
                    database_name=os.environ["DATABASE_NAME"],
                    mem0ai_user=MEM0AI_DATABASE_USER,
                    mem0ai_password=MEM0AI_DATABASE_ROLE_PASSWORD,  # type: ignore[arg-type]
                    history_db_path=MEM0_HISTORY_DB_PATH,
                )
                memory = await run_in_threadpool(Memory.from_config, cfg)
                app.state.memory_service = MemoryService(
                    memory, cfg["vector_store"]["config"]
                )
                logger.info("Memory service initialized (in-process)")
            except (MemoryConfigError, Exception) as e:
                app.state.memory_service = None
                logger.warning(f"Memory initialization failed: {e}")
        else:
            app.state.memory_service = None
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
