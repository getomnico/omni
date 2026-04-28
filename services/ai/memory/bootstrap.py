"""Build the mem0 config dict from in-process AI-service state.

Replaces the old two-step bootstrap (entrypoint.sh + /internal/memory/llm-config):
  1. LLM block: picks the admin-selected memory LLM, falls back to default.
  2. Embedder block: uses the admin-configured embedding provider,
     probing live dimensions when unset.
  3. Fingerprint: sha256({provider}:{model}:{dims})[:12] → collection name.
  4. Vector store config: connects as the restricted mem0ai role in the
     main omni DB.
"""
import hashlib
import logging
from typing import Any

from db.configuration import ConfigurationRepository
from db_config import get_embedding_config
from embeddings import EmbeddingProvider

logger = logging.getLogger(__name__)

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


class MemoryConfigError(RuntimeError):
    """Raised when the in-process mem0 config cannot be built."""


def _llm_block(provider) -> dict[str, Any]:
    ptype = getattr(provider, "provider_type", None)
    model = getattr(provider, "model_name", None)
    cfg: dict[str, Any] = {"model": model, "temperature": 0.2}

    client = getattr(provider, "client", None)
    api_key = getattr(client, "api_key", None) if client is not None else None
    if api_key:
        cfg["api_key"] = api_key

    if ptype == "openai_compatible":
        client_base = getattr(client, "base_url", None) if client is not None else None
        if client_base is not None:
            cfg["openai_base_url"] = str(client_base).rstrip("/")

    mem0_provider = _MEM0_LLM_MAP.get(ptype)
    if mem0_provider is None:
        raise MemoryConfigError(
            f"LLM provider '{ptype}' is not supported by the memory module"
        )
    return {"provider": mem0_provider, "config": cfg}


async def _probe_embedding_dims(provider: EmbeddingProvider) -> int | None:
    try:
        chunks = await provider.generate_embeddings("test", "query", None, "none")
        if chunks and chunks[0].embedding:
            return len(chunks[0].embedding)
    except Exception as e:
        logger.warning(f"Could not probe embedding dimensions: {e}")
    return None


def _embedder_block(embed_cfg, dims: int | None) -> dict[str, Any]:
    ptype = embed_cfg.provider
    mem0_provider = _MEM0_EMBEDDER_MAP.get(ptype)
    if mem0_provider is None:
        raise MemoryConfigError(
            f"Embedding provider '{ptype}' is not supported by the memory "
            "module. Configure an OpenAI, local (TEI), Jina, or Bedrock "
            "embedding provider in Admin → Embeddings."
        )

    cfg: dict[str, Any] = {"model": embed_cfg.model}
    resolved_dims = embed_cfg.dimensions or dims
    if resolved_dims:
        cfg["embedding_dims"] = resolved_dims

    if mem0_provider == "openai":
        # mem0's OpenAI embedder requires a non-empty api_key even for
        # unauthenticated local servers.
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


async def build_mem0_config(
    app_state,
    *,
    database_host: str,
    database_port: int,
    database_name: str,
    mem0ai_user: str,
    mem0ai_password: str,
    history_db_path: str = "/tmp/mem0_history.db",
) -> dict[str, Any]:
    """Assemble the mem0 config dict from in-process AI-service state."""
    models_dict = getattr(app_state, "models", {}) or {}
    if not models_dict:
        raise MemoryConfigError("No LLM models configured")

    config_repo = ConfigurationRepository()
    memory_cfg = await config_repo.get("memory_llm_id")
    memory_llm_id = memory_cfg.get("value") if memory_cfg else None

    default_id = getattr(app_state, "default_model_id", None)
    llm_provider = (
        (models_dict.get(memory_llm_id) if memory_llm_id else None)
        or (models_dict.get(default_id) if default_id else None)
        or next(iter(models_dict.values()), None)
    )
    if not llm_provider:
        raise MemoryConfigError("No LLM models configured")

    embed_cfg = await get_embedding_config()
    if embed_cfg is None:
        raise MemoryConfigError(
            "No embedding provider configured. Set one in Admin → Embeddings."
        )

    dims: int | None = None
    if not embed_cfg.dimensions:
        embedding_provider = getattr(app_state, "embedding_provider", None)
        if embedding_provider is not None:
            dims = await _probe_embedding_dims(embedding_provider)

    embedder_block = _embedder_block(embed_cfg, dims)

    # Fingerprint: change of provider, model, or dims → new collection.
    fp_str = "{provider}:{model}:{dims}".format(
        provider=embedder_block["provider"],
        model=embedder_block["config"].get("model", ""),
        dims=embedder_block["config"].get("embedding_dims", 0),
    )
    fp = hashlib.sha256(fp_str.encode()).hexdigest()[:12]
    collection_name = f"mem0_memories_{fp}"

    vector_store_config: dict[str, Any] = {
        "host":            database_host,
        "port":            database_port,
        "dbname":          database_name,
        "user":            mem0ai_user,
        "password":        mem0ai_password,
        "collection_name": collection_name,
    }
    if embedder_block["config"].get("embedding_dims"):
        vector_store_config["embedding_model_dims"] = (
            embedder_block["config"]["embedding_dims"]
        )

    return {
        "vector_store": {"provider": "pgvector", "config": vector_store_config},
        "llm":          _llm_block(llm_provider),
        "embedder":     embedder_block,
        "history_db_path": history_db_path,
    }
