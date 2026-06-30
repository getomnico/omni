import json
from dataclasses import dataclass
from datetime import datetime
from typing import Any, Literal

from asyncpg import Pool
from ulid import ULID

from .connection import get_db_pool

CompactionTargetType = Literal["chat", "agent_run"]


@dataclass(frozen=True)
class Compaction:
    id: str
    target_type: CompactionTargetType
    chat_id: str | None
    agent_run_id: str | None
    anchor_message_id: str | None
    anchor_log_id: str | None
    compacted_through_seq_num: int
    summary: str
    summary_message: dict[str, Any]
    previous_compaction_id: str | None
    estimated_input_tokens: int | None
    actual_input_tokens: int | None
    estimated_summary_tokens: int | None
    actual_summary_tokens: int | None
    metadata: dict[str, Any]
    created_at: datetime

    @classmethod
    def from_row(cls, row: dict[str, Any]) -> "Compaction":
        summary_message = row["summary_message"]
        if isinstance(summary_message, str):
            summary_message = json.loads(summary_message)
        metadata = row.get("metadata") or {}
        if isinstance(metadata, str):
            metadata = json.loads(metadata)
        return cls(
            id=row["id"].strip(),
            target_type=row["target_type"],
            chat_id=row["chat_id"].strip() if row.get("chat_id") else None,
            agent_run_id=(
                row["agent_run_id"].strip() if row.get("agent_run_id") else None
            ),
            anchor_message_id=(
                row["anchor_message_id"].strip()
                if row.get("anchor_message_id")
                else None
            ),
            anchor_log_id=(
                row["anchor_log_id"].strip() if row.get("anchor_log_id") else None
            ),
            compacted_through_seq_num=row["compacted_through_seq_num"],
            previous_compaction_id=(
                row["previous_compaction_id"].strip()
                if row.get("previous_compaction_id")
                else None
            ),
            summary=row["summary"],
            summary_message=summary_message,
            estimated_input_tokens=row.get("estimated_input_tokens"),
            actual_input_tokens=row.get("actual_input_tokens"),
            estimated_summary_tokens=row.get("estimated_summary_tokens"),
            actual_summary_tokens=row.get("actual_summary_tokens"),
            metadata=metadata,
            created_at=row["created_at"],
        )


CompactionRecord = Compaction
ChatCompaction = Compaction
AgentRunCompaction = Compaction

_COLUMNS = """
    id, target_type, chat_id, agent_run_id, anchor_message_id, anchor_log_id,
    compacted_through_seq_num, previous_compaction_id, summary, summary_message,
    estimated_input_tokens, actual_input_tokens,
    estimated_summary_tokens, actual_summary_tokens,
    metadata, created_at
"""


class CompactionsRepository:
    def __init__(self, pool: Pool | None = None):
        self.pool = pool

    async def _get_pool(self) -> Pool:
        if self.pool:
            return self.pool
        return await get_db_pool()

    async def get_latest_for_chat_path(
        self, chat_id: str, active_path_ids: list[str]
    ) -> Compaction | None:
        if not active_path_ids:
            return None
        pool = await self._get_pool()
        async with pool.acquire() as conn:
            row = await conn.fetchrow(
                f"""
                SELECT {_COLUMNS}
                FROM compactions
                WHERE target_type = 'chat'
                  AND chat_id = $1
                  AND anchor_message_id = ANY($2::varchar[])
                ORDER BY compacted_through_seq_num DESC, created_at DESC
                LIMIT 1
                """,
                chat_id,
                active_path_ids,
            )
        return Compaction.from_row(dict(row)) if row else None

    async def create_chat_compaction(
        self,
        *,
        chat_id: str,
        anchor_message_id: str,
        compacted_through_seq_num: int,
        summary: str,
        summary_message: dict[str, Any],
        previous_compaction_id: str | None = None,
        estimated_input_tokens: int | None = None,
        actual_input_tokens: int | None = None,
        estimated_summary_tokens: int | None = None,
        actual_summary_tokens: int | None = None,
        metadata: dict[str, Any] | None = None,
    ) -> Compaction:
        pool = await self._get_pool()
        async with pool.acquire() as conn:
            row = await conn.fetchrow(
                f"""
                INSERT INTO compactions (
                    id, target_type, chat_id, anchor_message_id,
                    compacted_through_seq_num, previous_compaction_id,
                    summary, summary_message, estimated_input_tokens,
                    actual_input_tokens, estimated_summary_tokens,
                    actual_summary_tokens, metadata
                )
                VALUES (
                    $1, 'chat', $2, $3, $4, $5, $6, $7::jsonb, $8, $9,
                    $10, $11, $12::jsonb
                )
                ON CONFLICT (anchor_message_id) WHERE anchor_message_id IS NOT NULL
                DO UPDATE SET anchor_message_id = EXCLUDED.anchor_message_id
                RETURNING {_COLUMNS}
                """,
                str(ULID()),
                chat_id,
                anchor_message_id,
                compacted_through_seq_num,
                previous_compaction_id,
                summary,
                json.dumps(summary_message),
                estimated_input_tokens,
                actual_input_tokens,
                estimated_summary_tokens,
                actual_summary_tokens,
                json.dumps(metadata or {}),
            )
        return Compaction.from_row(dict(row))

    async def get_latest_for_agent_run(self, run_id: str) -> Compaction | None:
        pool = await self._get_pool()
        async with pool.acquire() as conn:
            row = await conn.fetchrow(
                f"""
                SELECT {_COLUMNS}
                FROM compactions
                WHERE target_type = 'agent_run'
                  AND agent_run_id = $1
                ORDER BY compacted_through_seq_num DESC, created_at DESC
                LIMIT 1
                """,
                run_id,
            )
        return Compaction.from_row(dict(row)) if row else None

    async def create_agent_run_compaction(
        self,
        *,
        run_id: str,
        anchor_log_id: str,
        compacted_through_seq_num: int,
        summary: str,
        summary_message: dict[str, Any],
        previous_compaction_id: str | None = None,
        estimated_input_tokens: int | None = None,
        actual_input_tokens: int | None = None,
        estimated_summary_tokens: int | None = None,
        actual_summary_tokens: int | None = None,
        metadata: dict[str, Any] | None = None,
    ) -> Compaction:
        pool = await self._get_pool()
        async with pool.acquire() as conn:
            row = await conn.fetchrow(
                f"""
                INSERT INTO compactions (
                    id, target_type, agent_run_id, anchor_log_id,
                    compacted_through_seq_num, previous_compaction_id,
                    summary, summary_message, estimated_input_tokens,
                    actual_input_tokens, estimated_summary_tokens,
                    actual_summary_tokens, metadata
                )
                VALUES (
                    $1, 'agent_run', $2, $3, $4, $5, $6, $7::jsonb, $8,
                    $9, $10, $11, $12::jsonb
                )
                ON CONFLICT (anchor_log_id) WHERE anchor_log_id IS NOT NULL
                DO UPDATE SET anchor_log_id = EXCLUDED.anchor_log_id
                RETURNING {_COLUMNS}
                """,
                str(ULID()),
                run_id,
                anchor_log_id,
                compacted_through_seq_num,
                previous_compaction_id,
                summary,
                json.dumps(summary_message),
                estimated_input_tokens,
                actual_input_tokens,
                estimated_summary_tokens,
                actual_summary_tokens,
                json.dumps(metadata or {}),
            )
        return Compaction.from_row(dict(row))
