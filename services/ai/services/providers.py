"""Provider initialization and lifecycle management."""

import asyncio
import logging

import redis.asyncio as aioredis

from config import (
    AWS_REGION,
    EMBEDDING_MAX_MODEL_LEN,
    OPENAI_EMBEDDING_API_KEY,
    OPENAI_EMBEDDING_DIMENSIONS,
    LOCAL_EMBEDDINGS_URL,
    LOCAL_EMBEDDINGS_MODEL,
    REDIS_URL,
)
from db_config import (
    get_llm_config,
    get_embedding_config,
    VLLMConfig,
    AnthropicConfig,
    BedrockLLMConfig,
    LocalEmbeddingConfig,
    JinaEmbeddingConfig,
    OpenAIEmbeddingConfig,
    BedrockEmbeddingConfig,
)
from providers import create_llm_provider
from embeddings import create_embedding_provider
from tools import SearcherTool
from storage import create_content_storage
from embeddings.batch_processor import start_batch_processing

from state import AppState

logger = logging.getLogger(__name__)


async def initialize_providers(app_state: AppState) -> None:
    """
    Initialize all providers (embedding, LLM, tools, storage).

    Args:
        app_state: The FastAPI application state to populate

    Raises:
        Exception: If initialization fails
    """
    embedding_config = await get_embedding_config()
    logger.info(
        f"Loaded embedding configuration from database (provider: {embedding_config.provider})"
    )

    match embedding_config:
        case JinaEmbeddingConfig():
            if not embedding_config.jina_api_key:
                raise ValueError("JINA_API_KEY is required when using Jina provider")

            logger.info(
                f"Initializing JINA embedding provider with model: {embedding_config.jina_model}"
            )
            app_state.embedding_provider = create_embedding_provider(
                "jina",
                api_key=embedding_config.jina_api_key,
                model=embedding_config.jina_model,
                api_url=embedding_config.jina_api_url,
                max_model_len=EMBEDDING_MAX_MODEL_LEN,
            )

        case BedrockEmbeddingConfig():
            logger.info(
                f"Initializing Bedrock embedding provider with model: {embedding_config.bedrock_model_id}"
            )
            region_name = AWS_REGION if AWS_REGION else None
            app_state.embedding_provider = create_embedding_provider(
                "bedrock",
                model_id=embedding_config.bedrock_model_id,
                region_name=region_name,
                max_model_len=EMBEDDING_MAX_MODEL_LEN,
            )

        case OpenAIEmbeddingConfig():
            api_key = embedding_config.openai_api_key or OPENAI_EMBEDDING_API_KEY
            if not api_key:
                raise ValueError(
                    "OPENAI_EMBEDDING_API_KEY is required when using OpenAI provider"
                )

            model = embedding_config.openai_model
            dimensions = (
                embedding_config.openai_dimensions or OPENAI_EMBEDDING_DIMENSIONS
            )

            logger.info(f"Initializing OpenAI embedding provider with model: {model}")
            app_state.embedding_provider = create_embedding_provider(
                "openai",
                api_key=api_key,
                model=model,
                dimensions=dimensions,
                max_model_len=EMBEDDING_MAX_MODEL_LEN,
            )

        case LocalEmbeddingConfig():
            base_url = embedding_config.local_base_url or LOCAL_EMBEDDINGS_URL
            model = embedding_config.local_model or LOCAL_EMBEDDINGS_MODEL

            logger.info(
                f"Initializing local (vLLM) embedding provider with model: {model} at {base_url}"
            )
            app_state.embedding_provider = create_embedding_provider(
                "local",
                base_url=base_url,
                model=model,
                max_model_len=EMBEDDING_MAX_MODEL_LEN,
            )

        case _:
            raise ValueError(f"Unknown embedding provider: {embedding_config.provider}")

    logger.info(
        f"Initialized {embedding_config.provider} embedding provider with model: {app_state.embedding_provider.get_model_name()}"
    )

    # Initialize LLM provider using database configuration (with env fallback)
    llm_config = await get_llm_config()
    logger.info(
        f"Loaded LLM configuration from database (provider: {llm_config.provider})"
    )

    match llm_config:
        case VLLMConfig():
            app_state.llm_provider = create_llm_provider(
                "vllm", vllm_url=llm_config.vllm_url
            )
            logger.info(f"Initialized vLLM provider with URL: {llm_config.vllm_url}")

        case AnthropicConfig():
            app_state.llm_provider = create_llm_provider(
                "anthropic",
                api_key=llm_config.anthropic_api_key,
                model=llm_config.primary_model_id,
            )
            logger.info(
                f"Initialized Anthropic provider with model: {llm_config.primary_model_id}"
            )

        case BedrockLLMConfig():
            region_name = AWS_REGION if AWS_REGION else None
            app_state.llm_provider = create_llm_provider(
                "bedrock",
                model_id=llm_config.primary_model_id,
                secondary_model_id=llm_config.secondary_model_id,
                region_name=region_name,
            )
            logger.info(
                f"Initialized AWS Bedrock provider with model: {llm_config.primary_model_id}"
            )
            if llm_config.secondary_model_id:
                logger.info(f"Using secondary model: {llm_config.secondary_model_id}")
            if region_name:
                logger.info(f"Using AWS region: {region_name}")
            else:
                logger.info("Using auto-detected AWS region from ECS environment")

        case _:
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
    """
    Cleanup providers on shutdown.

    Args:
        app_state: The FastAPI application state
    """
    if app_state.redis_client:
        await app_state.redis_client.close()
        logger.info("Closed Redis client")
    logger.info("AI service shutdown complete")
