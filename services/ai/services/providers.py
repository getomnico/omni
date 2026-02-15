"""Provider initialization and lifecycle management."""

import asyncio
import logging

import redis.asyncio as aioredis

from config import (
    AWS_REGION,
    EMBEDDING_MAX_MODEL_LEN,
    REDIS_URL,
)
from db_config import (
    get_embedding_config,
)
from db import ModelsRepository, ModelRecord
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
            secondary_model_id=model_id,
            region_name=region_name,
        )

    elif provider_type == "openai":
        return create_llm_provider(
            "openai",
            api_key=config.get("apiKey"),
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

    for record in records:
        try:
            provider = _create_provider_from_model_record(record)
            models[record.id] = provider
            logger.info(
                f"Initialized model '{record.display_name}' (type={record.provider_type}, model={record.model_id}, id={record.id})"
            )
            if record.is_default:
                default_id = record.id
        except Exception as e:
            logger.error(
                f"Failed to initialize model '{record.display_name}' (id={record.id}): {e}"
            )

    app_state.models = models
    app_state.default_model_id = default_id

    if not models:
        logger.warning(
            "No models configured — chat will be unavailable until models are added"
        )
    else:
        logger.info(f"Loaded {len(models)} model(s), default={default_id}")


async def initialize_providers(app_state: AppState) -> None:
    """Initialize all providers (embedding, LLM, tools, storage)."""
    embedding_config = await get_embedding_config()
    provider = embedding_config.provider
    logger.info(f"Loaded embedding configuration (provider: {provider})")

    max_model_len = embedding_config.max_model_len or EMBEDDING_MAX_MODEL_LEN

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

    logger.info(
        f"Initialized {provider} embedding provider with model: {app_state.embedding_provider.get_model_name()}"
    )

    # Initialize models from database
    await load_models(app_state)

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
    embedding_config = await get_embedding_config()
    asyncio.create_task(
        start_batch_processing(
            app_state.content_storage,
            app_state.embedding_provider,
            embedding_config.provider,
        )
    )
    logger.info(
        f"Started embedding batch processing with provider: {embedding_config.provider}"
    )


async def shutdown_providers(app_state: "AppState"):
    """Cleanup providers on shutdown."""
    if app_state.redis_client:
        await app_state.redis_client.close()
        logger.info("Closed Redis client")
    logger.info("AI service shutdown complete")
