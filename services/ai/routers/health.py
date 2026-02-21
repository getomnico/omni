"""Health check endpoint."""

import logging

from fastapi import APIRouter, Request

from config import PORT
from db_config import get_embedding_config

logger = logging.getLogger(__name__)
router = APIRouter(tags=["health"])


@router.get("/health")
async def health_check(request: Request):
    """Health check endpoint."""
    # Check LLM provider health (check default model)
    llm_health = False
    llm_provider_name = "none"
    llm_model_name = "none"
    models = getattr(request.app.state, "models", {})
    if models:
        default_id = getattr(request.app.state, "default_model_id", None)
        provider = (
            models.get(default_id) if default_id else next(iter(models.values()), None)
        )
        if provider:
            llm_provider_name = type(provider).__name__
            llm_model_name = getattr(provider, "model", None) or getattr(
                provider, "model_id", "unknown"
            )
            try:
                llm_health = await provider.health_check()
            except Exception:
                llm_health = False

    # Get embedding model name from provider
    embedding_model = (
        request.app.state.embedding_provider.get_model_name()
        if hasattr(request.app.state, "embedding_provider")
        else "unknown"
    )

    # Get current configurations
    embedding_config = await get_embedding_config()

    return {
        "status": "healthy",
        "service": "ai",
        "embedding_provider": embedding_config.provider if embedding_config else "none",
        "embedding_model": embedding_model,
        "port": PORT,
        "embedding_dimensions": (
            embedding_config.dimensions if embedding_config else None
        ),
        "llm_provider": llm_provider_name,
        "llm_model": llm_model_name,
        "llm_health": llm_health,
    }
