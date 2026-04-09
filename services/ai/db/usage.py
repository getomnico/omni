from dataclasses import dataclass
from typing import Optional

from asyncpg import Pool
from ulid import ULID

from .connection import get_db_pool


@dataclass
class UsageSummary:
    model_name: str
    provider_type: str
    purpose: str
    call_count: int
    total_input_tokens: int
    total_output_tokens: int
    total_cache_read_tokens: int
    total_cache_creation_tokens: int


class UsageRepository:
    def __init__(self, pool: Optional[Pool] = None):
        self.pool = pool

    async def _get_pool(self) -> Pool:
        if self.pool:
            return self.pool
        return await get_db_pool()

    async def upsert(
        self,
        user_id: str | None,
        model_id: str,
        model_name: str,
        provider_type: str,
        purpose: str,
        input_tokens: int,
        output_tokens: int,
        cache_read_tokens: int = 0,
        cache_creation_tokens: int = 0,
        chat_id: str | None = None,
        agent_run_id: str | None = None,
    ) -> None:
        if chat_id is None and agent_run_id is None:
            raise ValueError("At least one of chat_id or agent_run_id must be set")

        pool = await self._get_pool()
        usage_id = str(ULID())

        if chat_id is not None:
            conflict_clause = "(chat_id, model_id, purpose) WHERE chat_id IS NOT NULL"
        else:
            conflict_clause = (
                "(agent_run_id, model_id, purpose) WHERE agent_run_id IS NOT NULL"
            )

        query = f"""
            INSERT INTO model_usage (
                id, user_id, model_id, model_name, provider_type,
                purpose, input_tokens, output_tokens,
                cache_read_tokens, cache_creation_tokens,
                chat_id, agent_run_id
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
            ON CONFLICT {conflict_clause}
            DO UPDATE SET
                input_tokens = model_usage.input_tokens + EXCLUDED.input_tokens,
                output_tokens = model_usage.output_tokens + EXCLUDED.output_tokens,
                cache_read_tokens = model_usage.cache_read_tokens + EXCLUDED.cache_read_tokens,
                cache_creation_tokens = model_usage.cache_creation_tokens + EXCLUDED.cache_creation_tokens,
                call_count = model_usage.call_count + 1,
                updated_at = NOW()
        """
        async with pool.acquire() as conn:
            await conn.execute(
                query,
                usage_id,
                user_id,
                model_id,
                model_name,
                provider_type,
                purpose,
                input_tokens,
                output_tokens,
                cache_read_tokens,
                cache_creation_tokens,
                chat_id,
                agent_run_id,
            )

    async def get_summary(
        self, days: int = 30, user_id: str | None = None
    ) -> list[UsageSummary]:
        pool = await self._get_pool()

        query = """
            SELECT model_name, provider_type, purpose,
                   SUM(call_count) as call_count,
                   SUM(input_tokens) as total_input_tokens,
                   SUM(output_tokens) as total_output_tokens,
                   SUM(cache_read_tokens) as total_cache_read_tokens,
                   SUM(cache_creation_tokens) as total_cache_creation_tokens
            FROM model_usage
            WHERE created_at >= NOW() - INTERVAL '1 day' * $1
        """
        params: list = [days]

        if user_id:
            query += " AND user_id = $2"
            params.append(user_id)

        query += " GROUP BY model_name, provider_type, purpose ORDER BY total_input_tokens DESC"

        async with pool.acquire() as conn:
            rows = await conn.fetch(query, *params)

        return [UsageSummary(**row) for row in rows]
