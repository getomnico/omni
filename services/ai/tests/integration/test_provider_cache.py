from __future__ import annotations

from dataclasses import dataclass

import pytest
from ulid import ULID

import db.connection
import provider_cache
from provider_cache import ProviderCache

pytestmark = pytest.mark.integration


@dataclass
class FakeProvider:
    model_id: str


@pytest.mark.asyncio
async def test_model_resolution_is_isolated_and_deleted_providers_are_unresolvable(
    db_pool, monkeypatch
):
    """Exercise DB resolution and caching with two models on one provider."""
    monkeypatch.setattr(db.connection, "_db_pool", db_pool)
    provider_id = str(ULID())
    first_model_id = str(ULID())
    second_model_id = str(ULID())

    async with db_pool.acquire() as conn:
        await conn.execute(
            """
            INSERT INTO model_providers (id, name, provider_type, config)
            VALUES ($1, $2, $3, $4::jsonb)
            """,
            provider_id,
            "cache test provider",
            "anthropic",
            "{}",
        )
        await conn.executemany(
            """
            INSERT INTO models (id, model_provider_id, model_id, display_name)
            VALUES ($1, $2, $3, $4)
            """,
            [
                (first_model_id, provider_id, "model-one", "Model One"),
                (second_model_id, provider_id, "model-two", "Model Two"),
            ],
        )

    monkeypatch.setattr(
        provider_cache,
        "_build_provider_from_record",
        lambda record: FakeProvider(record.model_id),
    )
    cache = ProviderCache()

    try:
        first = await cache.resolve_for_model(first_model_id)
        second = await cache.resolve_for_model(second_model_id)
        first_again = await cache.resolve_for_model(first_model_id)

        assert first is not None
        assert second is not None
        assert first.provider is not second.provider
        assert first.provider is first_again.provider
        assert first.model_name == "model-one"
        assert second.model_name == "model-two"

        async with db_pool.acquire() as conn:
            await conn.execute(
                "UPDATE models SET model_id = $1 WHERE id = $2",
                "model-one-updated",
                first_model_id,
            )
        first_after_model_update = await cache.resolve_for_model(first_model_id)
        assert first_after_model_update is not None
        assert first_after_model_update.provider is not first.provider
        assert first_after_model_update.model_name == "model-one-updated"

        cache.invalidate(provider_id)
        first_after_invalidation = await cache.resolve_for_model(first_model_id)
        assert first_after_invalidation is not None
        assert first_after_invalidation.provider is not first.provider

        async with db_pool.acquire() as conn:
            await conn.execute(
                "UPDATE model_providers SET is_deleted = TRUE WHERE id = $1",
                provider_id,
            )

        assert await cache.resolve_for_model(first_model_id) is None
    finally:
        async with db_pool.acquire() as conn:
            await conn.execute("DELETE FROM models WHERE model_provider_id = $1", provider_id)
            await conn.execute("DELETE FROM model_providers WHERE id = $1", provider_id)
