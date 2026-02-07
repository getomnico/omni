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
    get_llm_config,
    get_embedding_config,
    LLMConfig,
    EmbeddingConfig,
)
from providers import create_llm_provider
from embeddings import create_embedding_provider
from tools import SearcherTool
from storage import create_content_storage
from embeddings.batch_processor import start_batch_processing

from state import AppState

logger = logging.getLogger(__name__)


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

    # Initialize LLM provider
    llm_config = await get_llm_config()
    logger.info(f"Loaded LLM configuration (provider: {llm_config.provider})")

    if llm_config.provider == "vllm":
        app_state.llm_provider = create_llm_provider(
            "vllm", vllm_url=llm_config.api_url or ""
        )
        logger.info(f"Initialized vLLM provider with URL: {llm_config.api_url}")

    elif llm_config.provider == "anthropic":
        app_state.llm_provider = create_llm_provider(
            "anthropic",
            api_key=llm_config.api_key,
            model=llm_config.model,
        )
        logger.info(f"Initialized Anthropic provider with model: {llm_config.model}")

    elif llm_config.provider == "bedrock":
        region_name = AWS_REGION if AWS_REGION else None
        app_state.llm_provider = create_llm_provider(
            "bedrock",
            model_id=llm_config.model,
            secondary_model_id=llm_config.secondary_model,
            region_name=region_name,
        )
        logger.info(f"Initialized AWS Bedrock provider with model: {llm_config.model}")
        if llm_config.secondary_model:
            logger.info(f"Using secondary model: {llm_config.secondary_model}")
        if region_name:
            logger.info(f"Using AWS region: {region_name}")
        else:
            logger.info("Using auto-detected AWS region from ECS environment")

    else:
        raise ValueError(f"Unknown LLM provider: {llm_config.provider}")

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
