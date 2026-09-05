"""DB-backed provider resolution with cached SDK clients.

Replaces the old in-memory ``app_state.models`` dict that could silently
drift from the database.  On every request we read the model record (and
its provider row) from the DB.  The SDK client instance (httpx connection
pool) is cached by model record and reused across requests.

Staleness: the cached entry stores the model and provider ``updated_at``
timestamps. If either DB row is newer, the cached client is discarded and
rebuilt automatically.
"""

from __future__ import annotations

import asyncio
import logging
from dataclasses import dataclass
from datetime import datetime

from providers import LLMProvider
from db.model_providers import ModelsRepository, ModelRecord

logger = logging.getLogger(__name__)


@dataclass
class ResolvedModel:
    """Result of resolving a model record to a provider client.

    ``provider`` — the cached SDK client for this model record.
    ``model_record_id`` — the ``models.id`` to use in usage tracking.
    ``model_name`` — the wire model string (e.g. ``"claude-haiku-4-5"``)
    to pass into ``stream_response(…, model=…)``.
    """

    provider: LLMProvider
    model_record_id: str
    model_name: str


@dataclass
class _CachedEntry:
    provider: LLMProvider
    model_provider_id: str
    provider_updated_at: datetime | None
    model_updated_at: datetime


class ProviderCache:
    """Cache of LLMProvider SDK clients, keyed by model record ID.

    The database is the source of truth for *which models exist* and *which*
    model to use.  This cache only prevents re-building the httpx connection
    pool on every request.
    """

    def __init__(self) -> None:
        self._cache: dict[str, _CachedEntry] = {}
        self._lock = asyncio.Lock()

    def _cache_key(self, record: ModelRecord) -> str:
        return record.id

    async def resolve_for_model(self, model_record_id: str) -> ResolvedModel | None:
        """Resolve a model record id to a ``ResolvedModel``.

        Returns ``None`` when the model record is missing or its provider
        has been deleted (caller should fail loud).
        """
        repo = ModelsRepository()
        record = await repo.get(model_record_id)
        if record is None or record.is_deleted:
            return None
        provider = await self._get_or_build(record)
        if provider is None:
            return None
        return ResolvedModel(
            provider=provider,
            model_record_id=record.id,
            model_name=record.model_id,
        )

    async def resolve_default(self) -> ResolvedModel | None:
        """Resolve the default (is_default=True) model."""
        repo = ModelsRepository()
        record = await repo.get_default()
        if record is None:
            return None
        provider = await self._get_or_build(record)
        if provider is None:
            return None
        return ResolvedModel(
            provider=provider,
            model_record_id=record.id,
            model_name=record.model_id,
        )

    async def resolve_secondary_or_default(self) -> ResolvedModel | None:
        """Resolve the secondary model, falling back to default."""
        repo = ModelsRepository()
        record = await repo.get_secondary()
        if record is None:
            record = await repo.get_default()
        if record is None:
            return None
        provider = await self._get_or_build(record)
        if provider is None:
            return None
        return ResolvedModel(
            provider=provider,
            model_record_id=record.id,
            model_name=record.model_id,
        )

    async def _get_or_build(self, record: ModelRecord) -> LLMProvider | None:
        """Return a cached or freshly-built provider for a model record.

        Thread-safe: concurrent callers for the same model will serialise
        on ``_lock`` (intended for burst startup, not steady state — the
        first call builds the client; subsequent calls find a fresh entry).
        """
        key = self._cache_key(record)

        # Fast path — no lock.
        cached = self._cache.get(key)
        if cached is not None and _entry_is_fresh(cached, record):
            return cached.provider

        async with self._lock:
            # Double-check after acquiring the lock.
            cached = self._cache.get(key)
            if cached is not None and _entry_is_fresh(cached, record):
                return cached.provider

            provider = _build_provider_from_record(record)
            if provider is None:
                return None

            self._cache[key] = _CachedEntry(
                provider=provider,
                model_provider_id=record.model_provider_id,
                provider_updated_at=record.provider_updated_at,
                model_updated_at=record.updated_at,
            )
            logger.info(
                "Cached provider %s (type=%s, model=%s)",
                key, record.provider_type, record.model_id,
            )
            return provider

    def invalidate(self, model_provider_id: str) -> None:
        """Drop all cached models belonging to a provider configuration."""
        keys = [
            key
            for key, entry in self._cache.items()
            if entry.model_provider_id == model_provider_id
        ]
        for key in keys:
            self._cache.pop(key)

    def clear(self) -> None:
        """Drop all cached providers (e.g. on full reload)."""
        self._cache.clear()


def _entry_is_fresh(entry: _CachedEntry, record: ModelRecord) -> bool:
    """Return True when the cached entry is still up-to-date with the DB row."""
    if entry.provider_updated_at is None or record.provider_updated_at is None:
        # No timestamps available — assume stale and rebuild.
        return False
    return (
        entry.provider_updated_at == record.provider_updated_at
        and entry.model_updated_at == record.updated_at
    )


def _build_provider_from_record(record: ModelRecord) -> LLMProvider | None:
    """Instantiate an LLMProvider from a model+provider database record.

    Returns ``None`` on init failure (logged).  The caller should fall
    back to the next candidate instead of failing a chat request.
    """
    config = record.config
    provider_type = record.provider_type
    model_id = record.model_id

    try:
        if provider_type == "openai_compatible":
            base_url = config.get("apiUrl")
            if not base_url:
                raise ValueError("apiUrl is required in openai_compatible provider config")
            from providers.openai_compatible import OpenAICompatibleProvider
            return OpenAICompatibleProvider(
                base_url=base_url,
                api_key=config.get("apiKey"),
                model=model_id,
            )

        elif provider_type == "anthropic":
            from providers.anthropic import AnthropicProvider
            return AnthropicProvider(
                api_key=config.get("apiKey"),
                model=model_id,
            )

        elif provider_type == "bedrock":
            from config import AWS_REGION
            region_name = config.get("regionName") or AWS_REGION or None
            from providers.bedrock import BedrockProvider
            return BedrockProvider(
                model_id=model_id,
                region_name=region_name,
            )

        elif provider_type == "openai":
            from providers.openai import OpenAIProvider
            return OpenAIProvider(
                api_key=config.get("apiKey"),
                model=model_id,
            )

        elif provider_type == "gemini":
            from providers.gemini import GeminiProvider
            return GeminiProvider(
                api_key=config.get("apiKey"),
                model=model_id,
            )

        elif provider_type == "azure_foundry":
            from providers.azure_foundry import AzureFoundryProvider
            return AzureFoundryProvider(
                endpoint_url=config.get("apiUrl", ""),
                model=model_id,
            )

        elif provider_type == "vertex_ai":
            from providers.vertex_ai import VertexAIProvider
            return VertexAIProvider(
                region=config.get("regionName", ""),
                project_id=config.get("projectId", ""),
                model=model_id,
            )

        else:
            raise ValueError(f"Unknown provider type: {provider_type}")

    except Exception as e:
        logger.error(
            "Failed to build provider for '%s' (type=%s, id=%s): %s",
            record.display_name, provider_type, record.id, e,
        )
        return None
