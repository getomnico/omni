"""
Database-backed configuration for LLM and embedding settings.
Provides cached access to configuration stored in PostgreSQL,
with fallback to environment variables.
"""

import time
from typing import Optional
from dataclasses import dataclass

from config import (
    LLM_PROVIDER,
    LLM_API_KEY,
    LLM_MODEL,
    LLM_API_URL,
    LLM_SECONDARY_MODEL,
    DEFAULT_MAX_TOKENS,
    DEFAULT_TEMPERATURE,
    DEFAULT_TOP_P,
    EMBEDDING_PROVIDER,
    EMBEDDING_API_KEY,
    EMBEDDING_MODEL,
    EMBEDDING_API_URL,
    EMBEDDING_DIMENSIONS,
)
from db import fetch_llm_config, fetch_embedding_config


# =============================================================================
# LLM Configuration
# =============================================================================


@dataclass
class LLMConfig:
    provider: str
    api_key: Optional[str] = None
    model: str = ""
    api_url: Optional[str] = None
    secondary_model: Optional[str] = None
    max_tokens: Optional[int] = None
    temperature: Optional[float] = None
    top_p: Optional[float] = None


def parse_llm_config(data: dict) -> LLMConfig:
    provider = data.get("provider")
    if not provider:
        raise ValueError("LLM config missing 'provider' field")

    return LLMConfig(
        provider=provider,
        api_key=data.get("apiKey"),
        model=data.get("model", ""),
        api_url=data.get("apiUrl"),
        secondary_model=data.get("secondaryModel"),
        max_tokens=data.get("maxTokens"),
        temperature=data.get("temperature"),
        top_p=data.get("topP"),
    )


class LLMConfigCache:
    """Cached LLM configuration reader with PostgreSQL backend."""

    CACHE_TTL_SECONDS = 90

    def __init__(self):
        self._cache: Optional[LLMConfig] = None
        self._cache_timestamp: float = 0

    def _is_cache_valid(self) -> bool:
        if self._cache is None:
            return False
        elapsed = time.time() - self._cache_timestamp
        return elapsed < self.CACHE_TTL_SECONDS

    async def _fetch_from_database(self) -> Optional[LLMConfig]:
        config_data = await fetch_llm_config()
        if config_data:
            return parse_llm_config(config_data)
        return None

    def _get_env_fallback_config(self) -> LLMConfig:
        return LLMConfig(
            provider=LLM_PROVIDER,
            api_key=LLM_API_KEY or None,
            model=LLM_MODEL or "",
            api_url=LLM_API_URL or None,
            secondary_model=LLM_SECONDARY_MODEL or None,
            max_tokens=DEFAULT_MAX_TOKENS,
            temperature=DEFAULT_TEMPERATURE,
            top_p=DEFAULT_TOP_P,
        )

    async def get_config(self) -> LLMConfig:
        if self._is_cache_valid():
            return self._cache  # type: ignore

        db_config = await self._fetch_from_database()
        if db_config is not None:
            self._cache = db_config
            self._cache_timestamp = time.time()
            return db_config

        env_config = self._get_env_fallback_config()
        self._cache = env_config
        self._cache_timestamp = time.time()
        return env_config

    def invalidate_cache(self):
        self._cache = None
        self._cache_timestamp = 0


# Global instance
_llm_config_cache = LLMConfigCache()


async def get_llm_config() -> LLMConfig:
    return await _llm_config_cache.get_config()


def invalidate_llm_config_cache():
    _llm_config_cache.invalidate_cache()


# =============================================================================
# Embedding Configuration
# =============================================================================


@dataclass
class EmbeddingConfig:
    provider: str
    api_key: Optional[str] = None
    model: str = ""
    api_url: Optional[str] = None
    dimensions: Optional[int] = None


def parse_embedding_config(data: dict) -> EmbeddingConfig:
    provider = data.get("provider")
    if not provider:
        raise ValueError("Embedding config missing 'provider' field")

    return EmbeddingConfig(
        provider=provider,
        api_key=data.get("apiKey"),
        model=data.get("model", ""),
        api_url=data.get("apiUrl"),
        dimensions=data.get("dimensions"),
    )


class EmbeddingConfigCache:
    """Cached embedding configuration reader with PostgreSQL backend."""

    CACHE_TTL_SECONDS = 90

    def __init__(self):
        self._cache: Optional[EmbeddingConfig] = None
        self._cache_timestamp: float = 0

    def _is_cache_valid(self) -> bool:
        if self._cache is None:
            return False
        elapsed = time.time() - self._cache_timestamp
        return elapsed < self.CACHE_TTL_SECONDS

    async def _fetch_from_database(self) -> Optional[EmbeddingConfig]:
        config_data = await fetch_embedding_config()
        if config_data:
            return parse_embedding_config(config_data)
        return None

    def _get_env_fallback_config(self) -> EmbeddingConfig:
        return EmbeddingConfig(
            provider=EMBEDDING_PROVIDER,
            api_key=EMBEDDING_API_KEY or None,
            model=EMBEDDING_MODEL or "",
            api_url=EMBEDDING_API_URL or None,
            dimensions=EMBEDDING_DIMENSIONS,
        )

    async def get_config(self) -> EmbeddingConfig:
        if self._is_cache_valid():
            return self._cache  # type: ignore

        db_config = await self._fetch_from_database()
        if db_config is not None:
            self._cache = db_config
            self._cache_timestamp = time.time()
            return db_config

        env_config = self._get_env_fallback_config()
        self._cache = env_config
        self._cache_timestamp = time.time()
        return env_config

    def invalidate_cache(self):
        self._cache = None
        self._cache_timestamp = 0


# Global instance
_embedding_config_cache = EmbeddingConfigCache()


async def get_embedding_config() -> EmbeddingConfig:
    return await _embedding_config_cache.get_config()


def invalidate_embedding_config_cache():
    _embedding_config_cache.invalidate_cache()
