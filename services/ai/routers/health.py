"""Health check endpoint."""

import logging

from fastapi import APIRouter, Request

from config import PORT, EMBEDDING_DIMENSIONS
from db_config import get_llm_config, get_embedding_config

logger = logging.getLogger(__name__)
router = APIRouter(tags=["health"])


@router.get("/health")
async def health_check(request: Request):
    """Health check endpoint."""
    # Check LLM provider health
    llm_health = False
    if hasattr(request.app.state, "llm_provider") and request.app.state.llm_provider:
        try:
            llm_health = await request.app.state.llm_provider.health_check()
        except Exception:
            llm_health = False

    # Get embedding model name from provider
    embedding_model = (
        request.app.state.embedding_provider.get_model_name()
        if hasattr(request.app.state, "embedding_provider")
        else "unknown"
    )

    # Get current configurations
    llm_config = await get_llm_config()
    embedding_config = await get_embedding_config()

    return {
        "status": "healthy",
        "service": "ai",
        "embedding_provider": embedding_config.provider,
        "embedding_model": embedding_model,
        "port": PORT,
        "embedding_dimensions": EMBEDDING_DIMENSIONS,
        "llm_provider": llm_config.provider,
        "llm_model": llm_config.model or "default",
        "llm_health": llm_health,
    }
