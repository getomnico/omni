"""Internal endpoints — for service-to-service calls only, not browser-exposed."""

import logging
from typing import Any

from fastapi import APIRouter, HTTPException, Request

from db.configuration import ConfigurationRepository
from db_config import get_embedding_config
from embeddings import EmbeddingProvider

logger = logging.getLogger(__name__)
router = APIRouter(prefix="/internal", tags=["internal"])

# Omni LLM provider_type → mem0 LLM provider name
_MEM0_LLM_MAP = {
    "openai": "openai",
    "openai_compatible": "openai",
    "anthropic": "anthropic",
    "gemini": "gemini",
    "bedrock": "aws_bedrock",
    "aws_bedrock": "aws_bedrock",
}

# Omni embedding provider_type → mem0 embedder provider name
_MEM0_EMBEDDER_MAP = {
    "openai": "openai",
    "local": "openai",       # TEI / OpenAI-compatible local endpoint
    "jina": "openai",        # Jina exposes an OpenAI-compatible embeddings API
    "bedrock": "aws_bedrock",
}


def _llm_block(provider) -> dict[str, Any]:
    """Build mem0 LLM config from an omni LLM provider instance."""
    ptype = getattr(provider, "provider_type", None)
    model = getattr(provider, "model_name", None)
    cfg: dict[str, Any] = {"model": model, "temperature": 0.2}

    client = getattr(provider, "client", None)
    api_key = getattr(client, "api_key", None) if client is not None else None
    if api_key:
        cfg["api_key"] = api_key

    # openai_compatible stores the public base without /v1; the real v1 URL
    # lives on client.base_url. mem0's OpenAI SDK won't auto-append /v1.
    if ptype == "openai_compatible":
        client_base = getattr(client, "base_url", None) if client is not None else None
        if client_base is not None:
            cfg["openai_base_url"] = str(client_base).rstrip("/")

    mem0_provider = _MEM0_LLM_MAP.get(ptype)
    if mem0_provider is None:
        raise HTTPException(
            status_code=503,
            detail=f"LLM provider '{ptype}' is not supported by the memory service",
        )
    return {"provider": mem0_provider, "config": cfg}


async def _probe_embedding_dims(provider: EmbeddingProvider) -> int | None:
    """Return the dimension of the embedding provider by embedding a test string."""
    try:
        chunks = await provider.generate_embeddings("test", "query", None, "none")
        if chunks and chunks[0].embedding:
            return len(chunks[0].embedding)
    except Exception as e:
        logger.warning(f"Could not probe embedding dimensions: {e}")
    return None


def _embedder_block(
    embed_cfg, dims: int | None
) -> dict[str, Any]:
    """Build mem0 embedder config from the admin-configured embedding provider."""
    ptype = embed_cfg.provider
    mem0_provider = _MEM0_EMBEDDER_MAP.get(ptype)
    if mem0_provider is None:
        raise HTTPException(
            status_code=503,
            detail=(
                f"Embedding provider '{ptype}' is not supported by the memory "
                "service. Configure an OpenAI, local (TEI), Jina, or Bedrock "
                "embedding provider in Admin → Embeddings."
            ),
        )

    cfg: dict[str, Any] = {"model": embed_cfg.model}
    resolved_dims = embed_cfg.dimensions or dims
    if resolved_dims:
        cfg["embedding_dims"] = resolved_dims

    if mem0_provider == "openai":
        # mem0's OpenAI embedder requires a non-empty api_key even for local
        # OpenAI-compatible servers (TEI, vLLM without auth, etc.)
        cfg["api_key"] = embed_cfg.api_key or "unused"
        if ptype == "local" and embed_cfg.api_url:
            cfg["openai_base_url"] = embed_cfg.api_url.rstrip("/")
        elif ptype == "jina":
            cfg["openai_base_url"] = (
                (embed_cfg.api_url or "https://api.jina.ai/v1").rstrip("/")
            )
        elif ptype == "openai" and embed_cfg.api_url:
            cfg["openai_base_url"] = embed_cfg.api_url.rstrip("/")

    return {"provider": mem0_provider, "config": cfg}


@router.get("/memory/llm-config")
async def get_memory_llm_config(request: Request):
    """Return LLM and embedder config for the memory sidecar.

    LLM is the admin-selected `memory_llm_id` (falls back to the system default
    model). Embedder is the admin-configured embedding provider — the same one
    used for RAG indexing.
    """
    state = request.app.state
    models_dict = getattr(state, "models", {})
    if not models_dict:
        raise HTTPException(status_code=503, detail="No LLM models configured")

    config_repo = ConfigurationRepository()
    memory_cfg = await config_repo.get("memory_llm_id")
    memory_llm_id = memory_cfg.get("value") if memory_cfg else None

    default_id = getattr(state, "default_model_id", None)
    llm_provider = (
        (models_dict.get(memory_llm_id) if memory_llm_id else None)
        or (models_dict.get(default_id) if default_id else None)
        or next(iter(models_dict.values()), None)
    )
    if not llm_provider:
        raise HTTPException(status_code=503, detail="No LLM models configured")

    embed_cfg = await get_embedding_config()
    if embed_cfg is None:
        raise HTTPException(
            status_code=503,
            detail="No embedding provider configured. Set one in Admin → Embeddings.",
        )

    # Probe live dimensions if the admin didn't specify them (needed for pgvector
    # collection creation: mem0 defaults to 1536 which breaks local models).
    dims: int | None = None
    if not embed_cfg.dimensions:
        embedding_provider = getattr(state, "embedding_provider", None)
        if embedding_provider is not None:
            dims = await _probe_embedding_dims(embedding_provider)

    return {
        "llm": _llm_block(llm_provider),
        "embedder": _embedder_block(embed_cfg, dims),
    }
