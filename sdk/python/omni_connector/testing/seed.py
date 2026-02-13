"""SeedHelper — database seeding and querying for integration tests."""

from __future__ import annotations

import json
from typing import Any

import asyncpg
import ulid


def _new_ulid() -> str:
    return str(ulid.ULID())


class SeedHelper:
    """Database helpers for test setup and assertions. Bound to an asyncpg pool."""

    def __init__(self, pool: asyncpg.Pool) -> None:
        self._pool = pool

    async def create_user(
        self,
        user_id: str | None = None,
        email: str | None = None,
    ) -> str:
        user_id = user_id or _new_ulid()
        email = email or f"test-{user_id}@example.com"
        await self._pool.execute(
            """
            INSERT INTO users (id, email, password_hash, created_at, updated_at)
            VALUES ($1::char(26), $2, 'hash', NOW(), NOW())
            ON CONFLICT (id) DO NOTHING
            """,
            user_id,
            email,
        )
        return user_id

    async def create_source(
        self,
        source_type: str,
        config: dict[str, Any] | None = None,
        source_id: str | None = None,
        name: str | None = None,
        created_by: str | None = None,
    ) -> str:
        source_id = source_id or _new_ulid()
        name = name or f"Test {source_type} source"
        if created_by is None:
            created_by = await self.create_user()
        config_json = json.dumps(config or {})
        await self._pool.execute(
            """
            INSERT INTO sources (id, name, source_type, config, created_by, created_at, updated_at)
            VALUES ($1::char(26), $2, $3, $4::jsonb, $5::char(26), NOW(), NOW())
            ON CONFLICT (id) DO NOTHING
            """,
            source_id,
            name,
            source_type,
            config_json,
            created_by,
        )
        return source_id

    async def create_credentials(
        self,
        source_id: str,
        credentials: dict[str, Any],
        provider: str = "github",
        auth_type: str = "bearer_token",
    ) -> str:
        cred_id = _new_ulid()
        creds_json = json.dumps(credentials)
        await self._pool.execute(
            """
            INSERT INTO service_credentials
                (id, source_id, provider, auth_type, credentials, config, created_at, updated_at)
            VALUES ($1::char(26), $2::char(26), $3, $4, $5::jsonb, '{}'::jsonb, NOW(), NOW())
            """,
            cred_id,
            source_id,
            provider,
            auth_type,
            creds_json,
        )
        return cred_id

    async def get_sync_runs_for_source(self, source_id: str) -> list[asyncpg.Record]:
        return await self._pool.fetch(
            "SELECT * FROM sync_runs WHERE source_id = $1::char(26) ORDER BY created_at DESC",
            source_id,
        )

    async def get_events(self, source_id: str) -> list[asyncpg.Record]:
        return await self._pool.fetch(
            "SELECT * FROM connector_events_queue WHERE source_id = $1::char(26) ORDER BY created_at",
            source_id,
        )

    async def get_connector_state(self, source_id: str) -> Any:
        row = await self._pool.fetchrow(
            "SELECT connector_state FROM sources WHERE id = $1::char(26)",
            source_id,
        )
        if row and row["connector_state"]:
            return json.loads(row["connector_state"])
        return None

    async def get_sync_run(self, sync_run_id: str) -> asyncpg.Record | None:
        return await self._pool.fetchrow(
            "SELECT * FROM sync_runs WHERE id = $1::char(26)",
            sync_run_id,
        )

    async def cleanup_source(self, source_id: str) -> None:
        """Remove all data related to a source (for test isolation)."""
        await self._pool.execute(
            "DELETE FROM connector_events_queue WHERE source_id = $1::char(26)",
            source_id,
        )
        await self._pool.execute(
            "DELETE FROM sync_runs WHERE source_id = $1::char(26)", source_id
        )
        await self._pool.execute(
            "DELETE FROM service_credentials WHERE source_id = $1::char(26)", source_id
        )
        await self._pool.execute(
            "DELETE FROM sources WHERE id = $1::char(26)", source_id
        )
