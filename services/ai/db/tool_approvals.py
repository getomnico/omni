import json
from dataclasses import dataclass
from datetime import datetime
from enum import Enum
from typing import Any, Optional

from asyncpg import Pool
from ulid import ULID

from .connection import get_db_pool


class ToolApprovalType(str, Enum):
    APPROVAL = "approval"
    OAUTH = "oauth"


class ToolApprovalStatus(str, Enum):
    PENDING = "pending"
    APPROVED = "approved"
    DENIED = "denied"
    COMPLETED = "completed"
    EXPIRED = "expired"


@dataclass(frozen=True)
class ToolApproval:
    id: str
    chat_id: str
    user_id: str
    tool_name: str
    tool_input: dict[str, Any]
    status: ToolApprovalStatus
    created_at: datetime | None
    resolved_at: datetime | None
    resolved_by: str | None
    approval_type: ToolApprovalType
    tool_call_id: str | None
    source_id: str | None
    source_type: str | None
    provider: str | None
    oauth_start_url: str | None

    @classmethod
    def from_row(cls, row: dict[str, Any]) -> "ToolApproval":
        return cls(
            id=row["id"],
            chat_id=row["chat_id"],
            user_id=row["user_id"],
            tool_name=row["tool_name"],
            tool_input=_jsonb_value(row["tool_input"]),
            status=ToolApprovalStatus(row["status"]),
            created_at=row.get("created_at"),
            resolved_at=row.get("resolved_at"),
            resolved_by=row.get("resolved_by"),
            approval_type=ToolApprovalType(row["approval_type"]),
            tool_call_id=row.get("tool_call_id"),
            source_id=row.get("source_id"),
            source_type=row.get("source_type"),
            provider=row.get("provider"),
            oauth_start_url=row.get("oauth_start_url"),
        )


def _jsonb_value(value: Any) -> dict[str, Any]:
    if isinstance(value, str):
        decoded = json.loads(value)
    else:
        decoded = value
    if not isinstance(decoded, dict):
        raise ValueError("tool_input must be a JSON object")
    return decoded


class ToolApprovalsRepository:
    def __init__(self, pool: Optional[Pool] = None):
        self.pool = pool

    async def _get_pool(self) -> Pool:
        if self.pool:
            return self.pool
        return await get_db_pool()

    async def create_pending(
        self,
        *,
        chat_id: str,
        user_id: str,
        tool_name: str,
        tool_input: dict[str, Any],
        tool_call_id: str,
        approval_type: ToolApprovalType = ToolApprovalType.APPROVAL,
        source_id: str | None = None,
        source_type: str | None = None,
        provider: str | None = None,
        oauth_start_url: str | None = None,
    ) -> ToolApproval:
        approval_id = str(ULID())
        pool = await self._get_pool()
        async with pool.acquire() as conn:
            row = await conn.fetchrow(
                """
                INSERT INTO tool_approvals (
                    id,
                    chat_id,
                    user_id,
                    tool_name,
                    tool_input,
                    approval_type,
                    tool_call_id,
                    source_id,
                    source_type,
                    provider,
                    oauth_start_url
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
                RETURNING id, chat_id, user_id, tool_name, tool_input, status,
                          created_at, resolved_at, resolved_by, approval_type,
                          tool_call_id, source_id, source_type, provider, oauth_start_url
                """,
                approval_id,
                chat_id,
                user_id,
                tool_name,
                json.dumps(tool_input),
                approval_type.value,
                tool_call_id,
                source_id,
                source_type,
                provider,
                oauth_start_url,
            )

        return ToolApproval.from_row(dict(row))

    async def update_status(
        self,
        approval_id: str,
        status: ToolApprovalStatus,
        resolved_by: str | None = None,
    ) -> None:
        pool = await self._get_pool()
        async with pool.acquire() as conn:
            await conn.execute(
                """
                UPDATE tool_approvals
                SET status = $2,
                    resolved_at = CASE WHEN $2 = 'pending' THEN NULL ELSE NOW() END,
                    resolved_by = CASE
                        WHEN $2 = 'pending' THEN NULL
                        ELSE COALESCE($3, resolved_by)
                    END
                WHERE id = $1
                """,
                approval_id,
                status.value,
                resolved_by,
            )

    async def list_for_chat(
        self,
        *,
        chat_id: str,
        approval_type: ToolApprovalType,
        statuses: set[ToolApprovalStatus],
        active_tool_call_ids: set[str] | None = None,
    ) -> list[ToolApproval]:
        if active_tool_call_ids is not None and not active_tool_call_ids:
            return []

        pool = await self._get_pool()
        async with pool.acquire() as conn:
            rows = await conn.fetch(
                """
                SELECT id, chat_id, user_id, tool_name, tool_input, status,
                       created_at, resolved_at, resolved_by, approval_type,
                       tool_call_id, source_id, source_type, provider, oauth_start_url
                FROM tool_approvals
                WHERE chat_id = $1
                  AND approval_type = $2
                  AND status = ANY($3::text[])
                  AND ($4::text[] IS NULL OR tool_call_id = ANY($4::text[]))
                ORDER BY created_at ASC
                """,
                chat_id,
                approval_type.value,
                [status.value for status in statuses],
                (
                    list(active_tool_call_ids)
                    if active_tool_call_ids is not None
                    else None
                ),
            )

        return [ToolApproval.from_row(dict(row)) for row in rows]
