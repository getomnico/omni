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
from db import (
    EmbeddingProvidersRepository,
    WebFetchProvidersRepository,
    WebSearchProvidersRepository,
)
from db.listener import start_db_listener
from embeddings import create_embedding_provider
from tools import SearcherTool
from storage import create_content_storage
from embeddings.batch_processor import start_batch_processing
from web_providers import create_web_fetch_provider, create_web_search_provider

from state import AppState

logger = logging.getLogger(__name__)


async def load_models(app_state: AppState) -> None:
    """Reload model cache from the database.

    With DB-backed resolution the cache is populated on demand, so this
    just clears the cache to force a fresh fetch next time.
    """
    app_state.provider_cache.clear()
    logger.info("Provider cache cleared (will reload on next request)")


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
            api_url=embedding_config.api_url,
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


def _config_value(config: dict, key: str) -> str | None:
    value = config.get(key)
    if isinstance(value, str) and value:
        return value
    return None


async def _init_web_search_provider(app_state: AppState) -> None:
    repo = WebSearchProvidersRepository()
    record = await repo.get_current()
    if record is None:
        app_state.web_search_provider = None
        app_state.web_search_provider_type = None
        logger.info("No current web search provider configured")
        return

    app_state.web_search_provider = create_web_search_provider(
        record.provider_type,
        api_key=_config_value(record.config, "apiKey") or _config_value(record.config, "api_key"),
        base_url=_config_value(record.config, "baseUrl") or _config_value(record.config, "base_url"),
    )
    app_state.web_search_provider_type = record.provider_type
    logger.info(
        "Initialized web search provider '%s' (type=%s, id=%s)",
        record.name,
        record.provider_type,
        record.id,
    )


async def _init_web_fetch_provider(app_state: AppState) -> None:
    repo = WebFetchProvidersRepository()
    record = await repo.get_current()
    if record is None:
        app_state.web_fetch_provider = None
        app_state.web_fetch_provider_type = None
        logger.info("No current web fetch provider configured")
        return

    app_state.web_fetch_provider = create_web_fetch_provider(
        record.provider_type,
        api_key=_config_value(record.config, "apiKey") or _config_value(record.config, "api_key"),
        base_url=_config_value(record.config, "baseUrl") or _config_value(record.config, "base_url"),
    )
    app_state.web_fetch_provider_type = record.provider_type
    logger.info(
        "Initialized web fetch provider '%s' (type=%s, id=%s)",
        record.name,
        record.provider_type,
        record.id,
    )


async def reload_web_search_provider(app_state: AppState) -> None:
    await _init_web_search_provider(app_state)


async def reload_web_fetch_provider(app_state: AppState) -> None:
    await _init_web_fetch_provider(app_state)





def _handle_embedding_provider_notification(app_state: AppState, payload: dict) -> None:
    """Handle embedding_provider_changed notification — reload embedding provider."""
    logger.info(
        f"Embedding provider change detected via NOTIFY (id={payload.get('id')}), reloading"
    )
    asyncio.create_task(reload_embedding_provider(app_state))


def _handle_web_search_provider_notification(app_state: AppState, payload: dict) -> None:
    logger.info(
        "Web search provider change detected via NOTIFY (id=%s), reloading",
        payload.get("id"),
    )
    asyncio.create_task(reload_web_search_provider(app_state))


def _handle_web_fetch_provider_notification(app_state: AppState, payload: dict) -> None:
    logger.info(
        "Web fetch provider change detected via NOTIFY (id=%s), reloading",
        payload.get("id"),
    )
    asyncio.create_task(reload_web_fetch_provider(app_state))





async def initialize_providers(app_state: AppState) -> None:
    """Initialize all providers (embedding, LLM, tools, storage)."""
    await _init_embedding_provider(app_state)

    # Start DB listener for real-time config change notifications
    async def _on_reconnect():
        await reload_embedding_provider(app_state)
        await reload_web_search_provider(app_state)
        await reload_web_fetch_provider(app_state)

    app_state.listener_task = await start_db_listener(
        channels={
            "embedding_provider_changed": lambda payload: _handle_embedding_provider_notification(
                app_state, payload
            ),
            "web_search_provider_changed": lambda payload: _handle_web_search_provider_notification(
                app_state, payload
            ),
            "web_fetch_provider_changed": lambda payload: _handle_web_fetch_provider_notification(
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

    await _init_web_search_provider(app_state)
    await _init_web_fetch_provider(app_state)

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
