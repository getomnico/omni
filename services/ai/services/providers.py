"""Provider initialization and lifecycle management."""

import asyncio
import logging

import redis.asyncio as aioredis

from config import (
    AWS_REGION,
    REDIS_URL,
)
from db_config import (
    get_embedding_config,
    invalidate_embedding_config_cache,
)
from db import ModelsRepository, ModelRecord, EmbeddingProvidersRepository
from db.listener import start_db_listener
from providers import create_llm_provider, LLMProvider
from embeddings import create_embedding_provider
from tools import SearcherTool
from storage import create_content_storage
from embeddings.batch_processor import start_batch_processing

from state import AppState

logger = logging.getLogger(__name__)


def _create_provider_from_model_record(record: ModelRecord) -> LLMProvider:
    """Instantiate an LLMProvider from a model+provider database record."""
    config = record.config
    provider_type = record.provider_type
    model_id = record.model_id

    if provider_type == "vllm":
        return create_llm_provider("vllm", vllm_url=config.get("apiUrl", ""))

    elif provider_type == "anthropic":
        return create_llm_provider(
            "anthropic",
            api_key=config.get("apiKey"),
            model=model_id,
        )

    elif provider_type == "bedrock":
        region_name = config.get("regionName") or AWS_REGION or None
        return create_llm_provider(
            "bedrock",
            model_id=model_id,
            region_name=region_name,
        )

    elif provider_type == "openai":
        return create_llm_provider(
            "openai",
            api_key=config.get("apiKey"),
            model=model_id,
        )

    elif provider_type == "gemini":
        return create_llm_provider(
            "gemini",
            api_key=config.get("apiKey"),
            model=model_id,
        )

    elif provider_type == "azure_foundry":
        return create_llm_provider(
            "azure_foundry",
            endpoint_url=config.get("apiUrl", ""),
            model=model_id,
        )

    elif provider_type == "vertex_ai":
        return create_llm_provider(
            "vertex_ai",
            region=config.get("regionName", ""),
            project_id=config.get("projectId", ""),
            model=model_id,
        )

    else:
        raise ValueError(f"Unknown provider type: {provider_type}")


async def load_models(app_state: AppState) -> None:
    """Load all active models from the database and populate app_state."""
    repo = ModelsRepository()
    records = await repo.list_active()

    models: dict[str, LLMProvider] = {}
    default_id: str | None = None
    secondary_id: str | None = None

    for record in records:
        try:
            provider = _create_provider_from_model_record(record)
            models[record.id] = provider
            logger.info(
                f"Initialized model '{record.display_name}' (type={record.provider_type}, model={record.model_id}, id={record.id})"
            )
            if record.is_default:
                default_id = record.id
            if record.is_secondary:
                secondary_id = record.id
        except Exception as e:
            logger.error(
                f"Failed to initialize model '{record.display_name}' (id={record.id}): {e}"
            )

    app_state.models = models
    app_state.default_model_id = default_id
    app_state.secondary_model_id = secondary_id

    if not models:
        logger.warning(
            "No models configured — chat will be unavailable until models are added"
        )
    else:
        logger.info(
            f"Loaded {len(models)} model(s), default={default_id}, secondary={secondary_id}"
        )


async def _init_embedding_provider(app_state: AppState) -> None:
    """Initialize the embedding provider from current config."""
    repo = EmbeddingProvidersRepository()
    fingerprint = await repo.get_current_fingerprint()

    if fingerprint is None:
        app_state.embedding_provider = None
        app_state.embedding_provider_type = None
        app_state.embedding_provider_id = None
        app_state.embedding_provider_updated_at = None
        logger.warning("No current embedding provider configured")
        return

    app_state.embedding_provider_id = fingerprint[0]
    app_state.embedding_provider_updated_at = fingerprint[1]

    embedding_config = await get_embedding_config()
    provider = embedding_config.provider
    logger.info(f"Loaded embedding configuration (provider: {provider})")

    max_model_len = embedding_config.max_model_len or 8192

    if provider == "jina":
        if not embedding_config.api_key:
            raise ValueError("Embedding API key is required when using Jina provider")
        app_state.embedding_provider = create_embedding_provider(
            "jina",
            api_key=embedding_config.api_key,
            model=embedding_config.model,
            api_url=embedding_config.api_url,
            max_model_len=max_model_len,
        )

    elif provider == "bedrock":
        region_name = AWS_REGION if AWS_REGION else None
        app_state.embedding_provider = create_embedding_provider(
            "bedrock",
            model_id=embedding_config.model,
            region_name=region_name,
            max_model_len=max_model_len,
        )

    elif provider == "openai":
        if not embedding_config.api_key:
            raise ValueError("Embedding API key is required when using OpenAI provider")
        app_state.embedding_provider = create_embedding_provider(
            "openai",
            api_key=embedding_config.api_key,
            model=embedding_config.model,
            dimensions=embedding_config.dimensions,
            max_model_len=max_model_len,
        )

    elif provider == "cohere":
        if not embedding_config.api_key:
            raise ValueError("Embedding API key is required when using Cohere provider")
        app_state.embedding_provider = create_embedding_provider(
            "cohere",
            api_key=embedding_config.api_key,
            model=embedding_config.model,
            api_url=embedding_config.api_url,
            max_model_len=max_model_len,
            dimensions=embedding_config.dimensions,
        )

    elif provider == "local":
        app_state.embedding_provider = create_embedding_provider(
            "local",
            base_url=embedding_config.api_url or "",
            model=embedding_config.model,
            max_model_len=max_model_len,
        )

    else:
        raise ValueError(f"Unknown embedding provider: {provider}")

    app_state.embedding_provider_type = provider
    logger.info(
        f"Initialized {provider} embedding provider with model: {app_state.embedding_provider.get_model_name()}"
    )


async def reload_embedding_provider(app_state: AppState) -> None:
    """Re-read current embedding provider from DB and re-initialize.
    
    If a provider is newly configured (transitioning from None), also start
    the batch processor which may have exited early during startup.
    """
    was_none = app_state.embedding_provider is None
    invalidate_embedding_config_cache()
    await _init_embedding_provider(app_state)
    
    # Start batch processor if we just gained a provider
    if was_none and app_state.embedding_provider is not None:
        logger.info("Embedding provider became available, starting batch processor")
        await start_batch_processor(app_state)


def _handle_model_provider_notification(app_state: AppState, payload: dict) -> None:
    """Handle model_provider_changed notification — update default/secondary pointers."""
    model_id = payload.get("id", "").strip()
    if not model_id or model_id not in app_state.models:
        return

    if payload.get("is_default"):
        app_state.default_model_id = model_id
        logger.info(f"Default model updated to {model_id} via NOTIFY")
    elif app_state.default_model_id == model_id:
        app_state.default_model_id = None
        logger.info(f"Default model cleared (was {model_id}) via NOTIFY")

    if payload.get("is_secondary"):
        app_state.secondary_model_id = model_id
        logger.info(f"Secondary model updated to {model_id} via NOTIFY")
    elif app_state.secondary_model_id == model_id:
        app_state.secondary_model_id = None
        logger.info(f"Secondary model cleared (was {model_id}) via NOTIFY")


def _handle_embedding_provider_notification(app_state: AppState, payload: dict) -> None:
    """Handle embedding_provider_changed notification — reload embedding provider."""
    logger.info(
        f"Embedding provider change detected via NOTIFY (id={payload.get('id')}), reloading"
    )
    asyncio.create_task(reload_embedding_provider(app_state))


async def _refresh_model_flags(app_state: AppState) -> None:
    """Re-read default/secondary model flags from DB (catch-up after reconnect)."""
    repo = ModelsRepository()
    records = await repo.list_active()
    default_id: str | None = None
    secondary_id: str | None = None
    for record in records:
        if record.is_default:
            default_id = record.id
        if record.is_secondary:
            secondary_id = record.id
    app_state.default_model_id = default_id
    app_state.secondary_model_id = secondary_id
    logger.info(
        f"Refreshed model flags: default={default_id}, secondary={secondary_id}"
    )


async def initialize_providers(app_state: AppState) -> None:
    """Initialize all providers (embedding, LLM, tools, storage)."""
    await _init_embedding_provider(app_state)

    # Initialize models from database
    await load_models(app_state)

    # Start DB listener for real-time config change notifications
    async def _on_reconnect():
        await _refresh_model_flags(app_state)
        await reload_embedding_provider(app_state)

    app_state.listener_task = await start_db_listener(
        channels={
            "model_provider_changed": lambda payload: _handle_model_provider_notification(
                app_state, payload
            ),
            "embedding_provider_changed": lambda payload: _handle_embedding_provider_notification(
                app_state, payload
            ),
        },
        on_reconnect=_on_reconnect,
    )
    logger.info("Started DB change listener")

    # Initialize Redis client for caching
    app_state.redis_client = aioredis.from_url(REDIS_URL, decode_responses=True)
    logger.info(f"Initialized Redis client: {REDIS_URL}")

    # Initialize searcher client
    app_state.searcher_tool = SearcherTool()
    logger.info("Initialized searcher client")

    # Initialize content storage
    app_state.content_storage = create_content_storage()
    logger.info("Initialized content storage for batch processing")


async def start_batch_processor(app_state: AppState) -> None:
    """Start the embedding batch processor in the background."""
    asyncio.create_task(start_batch_processing(app_state))
    logger.info(
        f"Started embedding batch processing with provider: {app_state.embedding_provider_type}"
    )


async def shutdown_providers(app_state: "AppState"):
    """Cleanup providers on shutdown."""
    if app_state.listener_task:
        app_state.listener_task.cancel()
        logger.info("Cancelled DB listener task")
    if app_state.redis_client:
        await app_state.redis_client.close()
        logger.info("Closed Redis client")
    logger.info("AI service shutdown complete")
