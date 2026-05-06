"""Build the mem0 config from in-process AI-service state.

Steps:
  1. LLM block: picks the admin-selected memory LLM, falls back to default.
  2. Embedder block: uses the admin-configured embedding provider,
     probing live dimensions when unset.
  3. Fingerprint: sha256({provider}:{model}:{dims})[:12] → collection name.
  4. Vector store config: connects with the same DB role omni-ai uses
     for everything else (caller passes in the DB settings).

`build_mem0_config` returns mem0's typed `MemoryConfig` directly — no
intermediate dict shuffling.
"""

import hashlib
import logging
from dataclasses import dataclass
from urllib.parse import quote

from mem0.configs.base import MemoryConfig
from mem0.embeddings.configs import EmbedderConfig
from mem0.llms.configs import LlmConfig
from mem0.vector_stores.configs import VectorStoreConfig

from db.configuration import ConfigurationRepository
from db_config import EmbeddingConfig, get_embedding_config
from embeddings import EmbeddingProvider
from providers import LLMProvider
from state import AppState

logger = logging.getLogger(__name__)

# Postgres schema mem0 owns. Created by migration 090. Pinning the
# psycopg connection's search_path here lands every unqualified
# CREATE TABLE / CREATE INDEX mem0 emits inside this schema, keeping
# mem0's tables out of `public` alongside our app tables.
MEM0_SCHEMA = "mem0"

_MEM0_LLM_MAP = {
    "openai": "openai",
    "openai_compatible": "openai",
    "anthropic": "anthropic",
    "gemini": "gemini",
    "bedrock": "aws_bedrock",
    "aws_bedrock": "aws_bedrock",
}

_MEM0_EMBEDDER_MAP = {
    "openai": "openai",
    "local": "openai",  # TEI / OpenAI-compatible local endpoint
    "jina": "openai",  # Jina exposes an OpenAI-compatible embeddings API
    "bedrock": "aws_bedrock",
}


class MemoryConfigError(RuntimeError):
    """Raised when the in-process mem0 config cannot be built."""


@dataclass(frozen=True)
class DatabaseSettings:
    """Postgres connection settings the memory provider needs."""

    host: str
    port: int
    dbname: str
    user: str
    password: str


def _build_llm_block(provider: LLMProvider) -> LlmConfig:
    """Translate an `LLMProvider` into mem0's `LlmConfig`."""
    ptype = provider.provider_type
    mem0_provider = _MEM0_LLM_MAP.get(ptype) if ptype else None
    if mem0_provider is None:
        raise MemoryConfigError(
            f"LLM provider '{ptype}' is not supported by the memory module"
        )
    if not provider.model_name:
        raise MemoryConfigError(f"LLM provider '{ptype}' has no model_name set")

    inner: dict = {"model": provider.model_name, "temperature": 0.2}
    if provider.api_key:
        inner["api_key"] = provider.api_key
    if ptype == "openai_compatible" and provider.base_url:
        inner["openai_base_url"] = provider.base_url.rstrip("/")
    return LlmConfig(provider=mem0_provider, config=inner)


async def _probe_embedding_dims(provider: EmbeddingProvider) -> int | None:
    try:
        chunks = await provider.generate_embeddings("test", "query", None, "none")
        if chunks and chunks[0].embedding:
            return len(chunks[0].embedding)
    except Exception as e:
        logger.warning(f"Could not probe embedding dimensions: {e}")
    return None


def _build_embedder_block(
    embed_cfg: EmbeddingConfig, dims: int | None
) -> tuple[EmbedderConfig, int | None]:
    """Translate Omni's `EmbeddingConfig` into mem0's `EmbedderConfig`.

    Returns `(EmbedderConfig, embedding_dims)` so callers can stamp the
    same dim count onto the vector-store config.
    """
    ptype = embed_cfg.provider
    mem0_provider = _MEM0_EMBEDDER_MAP.get(ptype)
    if mem0_provider is None:
        raise MemoryConfigError(
            f"Embedding provider '{ptype}' is not supported by the memory "
            "module. Configure an OpenAI, local (TEI), Jina, or Bedrock "
            "embedding provider in Admin → Embeddings."
        )

    embedding_dims = embed_cfg.dimensions or dims
    inner: dict = {"model": embed_cfg.model}
    if embedding_dims:
        inner["embedding_dims"] = embedding_dims

    if mem0_provider == "openai":
        inner["api_key"] = embed_cfg.api_key or "unused"
        if ptype == "local" and embed_cfg.api_url:
            inner["openai_base_url"] = embed_cfg.api_url.rstrip("/")
        elif ptype == "jina":
            inner["openai_base_url"] = (
                embed_cfg.api_url or "https://api.jina.ai/v1"
            ).rstrip("/")
        elif ptype == "openai" and embed_cfg.api_url:
            inner["openai_base_url"] = embed_cfg.api_url.rstrip("/")

    return EmbedderConfig(provider=mem0_provider, config=inner), embedding_dims


def _pick_memory_llm(
    models: dict[str, LLMProvider],
    memory_llm_id: str | None,
    default_id: str | None,
) -> LLMProvider:
    """Resolve which LLM to use for memory extraction."""
    candidate = (
        (models.get(memory_llm_id) if memory_llm_id else None)
        or (models.get(default_id) if default_id else None)
        or next(iter(models.values()), None)
    )
    if candidate is None:
        raise MemoryConfigError("No LLM models configured")
    return candidate


async def build_mem0_config(
    app_state: AppState,
    *,
    db: DatabaseSettings,
    history_db_path: str,
) -> MemoryConfig:
    """Assemble mem0's `MemoryConfig` from in-process AI-service state."""
    if not app_state.models:
        raise MemoryConfigError("No LLM models configured")

    config_repo = ConfigurationRepository()
    memory_cfg = await config_repo.get_global("memory_llm_id")
    memory_llm_id = memory_cfg.get("value") if memory_cfg else None

    llm_provider = _pick_memory_llm(
        app_state.models, memory_llm_id, app_state.default_model_id
    )

    embed_cfg = await get_embedding_config()
    if embed_cfg is None:
        raise MemoryConfigError(
            "No embedding provider configured. Set one in Admin → Embeddings."
        )

    dims: int | None = None
    if not embed_cfg.dimensions and app_state.embedding_provider is not None:
        dims = await _probe_embedding_dims(app_state.embedding_provider)

    embedder_block, embedding_dims = _build_embedder_block(embed_cfg, dims)
    llm_block = _build_llm_block(llm_provider)

    fp_str = f"{embedder_block.provider}:{embed_cfg.model}:{embedding_dims or 0}"
    fp = hashlib.sha256(fp_str.encode()).hexdigest()[:12]
    collection_name = f"mem0_memories_{fp}"

    # mem0's PGVector emits unqualified DDL; pinning search_path via the
    # libpq `options` parameter routes every CREATE TABLE/INDEX into the
    # mem0 schema. Passing `connection_string` makes mem0 skip the
    # individual host/port/user/password kwargs (PGVector init priority:
    # connection_pool > connection_string > individual params), and its
    # PGVectorConfig validator skips the host/port/user/password presence
    # check when connection_string is set.
    #
    # `public` is kept in the search_path so the `vector` extension's
    # type (installed in `public`) resolves for unqualified column type
    # references like `vector(1536)`. `mem0` comes first so unqualified
    # CREATE TABLE/INDEX still lands in the mem0 schema.
    #
    # We use `quote` (not `quote_plus`) because libpq's URI parser does
    # not decode `+` to space -- it only decodes `%xx`. quote_plus would
    # encode the space in `-c search_path=...` as `+`, which Postgres
    # would then read as a parameter name starting with `+`.
    #
    # Caveat: PGVector.list_cols() hardcodes WHERE table_schema='public',
    # so it returns empty here and create_col() runs on every boot. All
    # statements in create_col() are IF NOT EXISTS, so the cost is one
    # no-op DDL transaction per AI-service start -- acceptable.
    options = quote(f"-c search_path={MEM0_SCHEMA},public", safe="")
    conn_str = (
        f"postgresql://{quote(db.user, safe='')}:{quote(db.password, safe='')}"
        f"@{db.host}:{db.port}/{quote(db.dbname, safe='')}"
        f"?options={options}"
    )
    pg_inner: dict = {
        "connection_string": conn_str,
        "collection_name": collection_name,
    }
    if embedding_dims:
        pg_inner["embedding_model_dims"] = embedding_dims
    vector_store = VectorStoreConfig(provider="pgvector", config=pg_inner)

    return MemoryConfig(
        vector_store=vector_store,
        llm=llm_block,
        embedder=embedder_block,
        history_db_path=history_db_path,
    )
