"""
Database-backed configuration for LLM and embedding settings.
Provides cached access to configuration stored in PostgreSQL,
with fallback to environment variables.
"""

import time
from typing import Optional, Union, Literal
from dataclasses import dataclass

from config import (
    LLM_PROVIDER,
    VLLM_URL,
    ANTHROPIC_API_KEY,
    ANTHROPIC_MODEL,
    BEDROCK_MODEL_ID,
    TITLE_GENERATION_MODEL_ID,
    DEFAULT_MAX_TOKENS,
    DEFAULT_TEMPERATURE,
    DEFAULT_TOP_P,
    EMBEDDING_PROVIDER,
    JINA_API_KEY,
    JINA_MODEL,
    JINA_API_URL,
    BEDROCK_EMBEDDING_MODEL_ID,
)
from db import fetch_llm_config, fetch_embedding_config


# =============================================================================
# LLM Configuration Types
# =============================================================================


@dataclass
class VLLMConfig:
    """vLLM provider configuration"""

    provider: Literal["vllm"]
    vllm_url: str
    primary_model_id: Optional[str]
    secondary_model_id: Optional[str]
    max_tokens: Optional[int]
    temperature: Optional[float]
    top_p: Optional[float]


@dataclass
class AnthropicConfig:
    """Anthropic provider configuration"""

    provider: Literal["anthropic"]
    anthropic_api_key: str
    primary_model_id: str
    secondary_model_id: Optional[str]
    max_tokens: Optional[int]
    temperature: Optional[float]
    top_p: Optional[float]


@dataclass
class BedrockLLMConfig:
    """AWS Bedrock LLM provider configuration"""

    provider: Literal["bedrock"]
    primary_model_id: str
    secondary_model_id: Optional[str]
    max_tokens: Optional[int]
    temperature: Optional[float]
    top_p: Optional[float]


LLMConfig = Union[VLLMConfig, AnthropicConfig, BedrockLLMConfig]


def parse_llm_config(data: dict) -> LLMConfig:
    """Parse raw dict to typed LLM config based on provider"""
    provider = data.get("provider")

    if provider == "vllm":
        return VLLMConfig(
            provider="vllm",
            vllm_url=data.get("vllmUrl", ""),
            primary_model_id=data.get("primaryModelId"),
            secondary_model_id=data.get("secondaryModelId"),
            max_tokens=data.get("maxTokens"),
            temperature=data.get("temperature"),
            top_p=data.get("topP"),
        )
    elif provider == "anthropic":
        return AnthropicConfig(
            provider="anthropic",
            anthropic_api_key=data.get("anthropicApiKey", ""),
            primary_model_id=data.get("primaryModelId", ""),
            secondary_model_id=data.get("secondaryModelId"),
            max_tokens=data.get("maxTokens"),
            temperature=data.get("temperature"),
            top_p=data.get("topP"),
        )
    elif provider == "bedrock":
        return BedrockLLMConfig(
            provider="bedrock",
            primary_model_id=data.get("primaryModelId", ""),
            secondary_model_id=data.get("secondaryModelId"),
            max_tokens=data.get("maxTokens"),
            temperature=data.get("temperature"),
            top_p=data.get("topP"),
        )
    else:
        raise ValueError(f"Unknown LLM provider: {provider}")


class LLMConfigCache:
    """
    Cached LLM configuration reader with PostgreSQL backend.
    Cache has a TTL of 90 seconds (1.5 minutes) to balance freshness with performance.
    """

    CACHE_TTL_SECONDS = 90  # 1.5 minutes

    def __init__(self):
        self._cache: Optional[LLMConfig] = None
        self._cache_timestamp: float = 0

    def _is_cache_valid(self) -> bool:
        """Check if cache is still valid"""
        if self._cache is None:
            return False
        elapsed = time.time() - self._cache_timestamp
        return elapsed < self.CACHE_TTL_SECONDS

    async def _fetch_from_database(self) -> Optional[LLMConfig]:
        """Fetch LLM configuration from database"""
        config_data = await fetch_llm_config()

        if config_data:
            return parse_llm_config(config_data)
        return None

    def _get_env_fallback_config(self) -> LLMConfig:
        """Get configuration from environment variables as fallback"""
        if LLM_PROVIDER == "vllm":
            return VLLMConfig(
                provider="vllm",
                vllm_url=VLLM_URL or "",
                primary_model_id=None,
                secondary_model_id=None,
                max_tokens=DEFAULT_MAX_TOKENS,
                temperature=DEFAULT_TEMPERATURE,
                top_p=DEFAULT_TOP_P,
            )
        elif LLM_PROVIDER == "anthropic":
            return AnthropicConfig(
                provider="anthropic",
                anthropic_api_key=ANTHROPIC_API_KEY or "",
                primary_model_id=ANTHROPIC_MODEL or "",
                secondary_model_id=None,
                max_tokens=DEFAULT_MAX_TOKENS,
                temperature=DEFAULT_TEMPERATURE,
                top_p=DEFAULT_TOP_P,
            )
        elif LLM_PROVIDER == "bedrock":
            return BedrockLLMConfig(
                provider="bedrock",
                primary_model_id=BEDROCK_MODEL_ID or "",
                secondary_model_id=TITLE_GENERATION_MODEL_ID,
                max_tokens=DEFAULT_MAX_TOKENS,
                temperature=DEFAULT_TEMPERATURE,
                top_p=DEFAULT_TOP_P,
            )
        else:
            raise ValueError(f"Unknown LLM provider from env: {LLM_PROVIDER}")

    async def get_config(self) -> LLMConfig:
        """
        Get LLM configuration with caching.
        Priority: Database config -> Environment variables
        """
        # Return cached config if still valid
        if self._is_cache_valid():
            return self._cache  # type: ignore

        # Try to fetch from database
        db_config = await self._fetch_from_database()

        if db_config is not None:
            # Use database configuration
            self._cache = db_config
            self._cache_timestamp = time.time()
            return db_config

        # Fall back to environment variables
        env_config = self._get_env_fallback_config()
        self._cache = env_config
        self._cache_timestamp = time.time()
        return env_config

    def invalidate_cache(self):
        """Manually invalidate the cache"""
        self._cache = None
        self._cache_timestamp = 0


# Global instance
_llm_config_cache = LLMConfigCache()


async def get_llm_config() -> LLMConfig:
    """Get current LLM configuration (with caching)"""
    return await _llm_config_cache.get_config()


def invalidate_llm_config_cache():
    """Invalidate the LLM configuration cache"""
    _llm_config_cache.invalidate_cache()


# =============================================================================
# Embedding Configuration Types
# =============================================================================


@dataclass
class LocalEmbeddingConfig:
    """Local (vLLM-based) embedding provider configuration"""

    provider: Literal["local"]
    local_base_url: str
    local_model: str


@dataclass
class JinaEmbeddingConfig:
    """Jina embedding provider configuration"""

    provider: Literal["jina"]
    jina_api_key: Optional[str]
    jina_model: str
    jina_api_url: Optional[str]


@dataclass
class OpenAIEmbeddingConfig:
    """OpenAI embedding provider configuration"""

    provider: Literal["openai"]
    openai_api_key: Optional[str]
    openai_model: str
    openai_dimensions: Optional[int]


@dataclass
class CohereEmbeddingConfig:
    """Cohere embedding provider configuration"""

    provider: Literal["cohere"]
    cohere_api_key: Optional[str]
    cohere_model: str
    cohere_api_url: Optional[str]
    cohere_dimensions: Optional[int]


@dataclass
class BedrockEmbeddingConfig:
    """AWS Bedrock embedding provider configuration"""

    provider: Literal["bedrock"]
    bedrock_model_id: str


EmbeddingConfig = Union[
    LocalEmbeddingConfig,
    JinaEmbeddingConfig,
    OpenAIEmbeddingConfig,
    CohereEmbeddingConfig,
    BedrockEmbeddingConfig,
]


def parse_embedding_config(data: dict) -> EmbeddingConfig:
    """Parse raw dict to typed embedding config based on provider"""
    provider = data.get("provider")

    if provider == "local":
        return LocalEmbeddingConfig(
            provider="local",
            local_base_url=data.get("localBaseUrl", ""),
            local_model=data.get("localModel", ""),
        )
    elif provider == "jina":
        return JinaEmbeddingConfig(
            provider="jina",
            jina_api_key=data.get("jinaApiKey"),
            jina_model=data.get("jinaModel", ""),
            jina_api_url=data.get("jinaApiUrl"),
        )
    elif provider == "openai":
        return OpenAIEmbeddingConfig(
            provider="openai",
            openai_api_key=data.get("openaiApiKey"),
            openai_model=data.get("openaiModel", ""),
            openai_dimensions=data.get("openaiDimensions"),
        )
    elif provider == "cohere":
        return CohereEmbeddingConfig(
            provider="cohere",
            cohere_api_key=data.get("cohereApiKey"),
            cohere_model=data.get("cohereModel", ""),
            cohere_api_url=data.get("cohereApiUrl"),
            cohere_dimensions=data.get("cohereDimensions"),
        )
    elif provider == "bedrock":
        return BedrockEmbeddingConfig(
            provider="bedrock",
            bedrock_model_id=data.get("bedrockModelId", ""),
        )
    else:
        raise ValueError(f"Unknown embedding provider: {provider}")


class EmbeddingConfigCache:
    """
    Cached embedding configuration reader with PostgreSQL backend.
    Cache has a TTL of 90 seconds (1.5 minutes) to balance freshness with performance.
    """

    CACHE_TTL_SECONDS = 90  # 1.5 minutes

    def __init__(self):
        self._cache: Optional[EmbeddingConfig] = None
        self._cache_timestamp: float = 0

    def _is_cache_valid(self) -> bool:
        """Check if cache is still valid"""
        if self._cache is None:
            return False
        elapsed = time.time() - self._cache_timestamp
        return elapsed < self.CACHE_TTL_SECONDS

    async def _fetch_from_database(self) -> Optional[EmbeddingConfig]:
        """Fetch embedding configuration from database"""
        config_data = await fetch_embedding_config()

        if config_data:
            return parse_embedding_config(config_data)
        return None

    def _get_env_fallback_config(self) -> EmbeddingConfig:
        """Get configuration from environment variables as fallback"""
        from config import (
            OPENAI_EMBEDDING_API_KEY,
            OPENAI_EMBEDDING_MODEL,
            OPENAI_EMBEDDING_DIMENSIONS,
            COHERE_EMBEDDING_API_KEY,
            COHERE_EMBEDDING_MODEL,
            COHERE_EMBEDDING_API_URL,
            COHERE_EMBEDDING_DIMENSIONS,
            LOCAL_EMBEDDINGS_URL,
            LOCAL_EMBEDDINGS_MODEL,
        )

        if EMBEDDING_PROVIDER == "local":
            return LocalEmbeddingConfig(
                provider="local",
                local_base_url=LOCAL_EMBEDDINGS_URL or "",
                local_model=LOCAL_EMBEDDINGS_MODEL or "",
            )
        elif EMBEDDING_PROVIDER == "jina":
            return JinaEmbeddingConfig(
                provider="jina",
                jina_api_key=JINA_API_KEY,
                jina_model=JINA_MODEL or "",
                jina_api_url=JINA_API_URL,
            )
        elif EMBEDDING_PROVIDER == "openai":
            return OpenAIEmbeddingConfig(
                provider="openai",
                openai_api_key=OPENAI_EMBEDDING_API_KEY,
                openai_model=OPENAI_EMBEDDING_MODEL or "",
                openai_dimensions=OPENAI_EMBEDDING_DIMENSIONS,
            )
        elif EMBEDDING_PROVIDER == "cohere":
            return CohereEmbeddingConfig(
                provider="cohere",
                cohere_api_key=COHERE_EMBEDDING_API_KEY,
                cohere_model=COHERE_EMBEDDING_MODEL or "",
                cohere_api_url=COHERE_EMBEDDING_API_URL,
                cohere_dimensions=COHERE_EMBEDDING_DIMENSIONS or None,
            )
        elif EMBEDDING_PROVIDER == "bedrock":
            return BedrockEmbeddingConfig(
                provider="bedrock",
                bedrock_model_id=BEDROCK_EMBEDDING_MODEL_ID or "",
            )
        else:
            raise ValueError(
                f"Unknown embedding provider from env: {EMBEDDING_PROVIDER}"
            )

    async def get_config(self) -> EmbeddingConfig:
        """
        Get embedding configuration with caching.
        Priority: Database config -> Environment variables
        """
        # Return cached config if still valid
        if self._is_cache_valid():
            return self._cache  # type: ignore

        # Try to fetch from database
        db_config = await self._fetch_from_database()

        if db_config is not None:
            # Use database configuration
            self._cache = db_config
            self._cache_timestamp = time.time()
            return db_config

        # Fall back to environment variables
        env_config = self._get_env_fallback_config()
        self._cache = env_config
        self._cache_timestamp = time.time()
        return env_config

    def invalidate_cache(self):
        """Manually invalidate the cache"""
        self._cache = None
        self._cache_timestamp = 0


# Global instance
_embedding_config_cache = EmbeddingConfigCache()


async def get_embedding_config() -> EmbeddingConfig:
    """Get current embedding configuration (with caching)"""
    return await _embedding_config_cache.get_config()


def invalidate_embedding_config_cache():
    """Invalidate the embedding configuration cache"""
    _embedding_config_cache.invalidate_cache()
