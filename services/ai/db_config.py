"""
Database-backed configuration for embedding settings.
Provides cached access to configuration stored in PostgreSQL.
"""

import time
from typing import Optional
from dataclasses import dataclass

from db import EmbeddingProvidersRepository


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
    max_model_len: Optional[int] = None


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
        repo = EmbeddingProvidersRepository()
        record = await repo.get_current()
        if record is None:
            return None

        config = record.config
        return EmbeddingConfig(
            provider=record.provider_type,
            api_key=config.get("apiKey"),
            model=config.get("model", ""),
            api_url=config.get("apiUrl"),
            dimensions=config.get("dimensions"),
            max_model_len=config.get("maxModelLen"),
        )

    async def get_config(self) -> Optional[EmbeddingConfig]:
        if self._is_cache_valid():
            return self._cache

        db_config = await self._fetch_from_database()
        if db_config is not None:
            self._cache = db_config
            self._cache_timestamp = time.time()
            return db_config

        return None

    def invalidate_cache(self):
        self._cache = None
        self._cache_timestamp = 0


# Global instance
_embedding_config_cache = EmbeddingConfigCache()


async def get_embedding_config() -> Optional[EmbeddingConfig]:
    return await _embedding_config_cache.get_config()


def invalidate_embedding_config_cache():
    _embedding_config_cache.invalidate_cache()
