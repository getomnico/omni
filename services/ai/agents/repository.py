"""Database access for agents and the durable agent_runs queue."""

from __future__ import annotations

import hashlib
import json
import logging
from datetime import datetime, timedelta
from typing import Optional, Sequence

from asyncpg import Pool
from ulid import ULID

from config import AGENT_RUN_MAX_ATTEMPTS
from db.connection import get_db_pool
from .models import (
    Agent,
    AgentRun,
    AgentRunAlreadyActive,
    AgentRunClaim,
    AgentRunCreateResult,
    AgentRunLog,
    AgentRunLogMessage,
    AgentRunRetryPolicy,
    AgentRunTriggerType,
    RunStatus,
)

logger = logging.getLogger(__name__)

_AGENT_RUN_COLUMNS = """
    id, agent_id, status, trigger_type, started_at, completed_at,
    claim_token, lease_expires_at, heartbeat_at, attempt_count, max_attempts,
    summary, error_message, created_at
"""

_AGENT_RUN_CLAIM_LOCK_KEY = 0x0A6E7A5F001  # arbitrary stable advisory-lock key


def _advisory_lock_key(value: str) -> int:
    digest = hashlib.sha256(value.encode("utf-8")).digest()[:8]
    return int.from_bytes(digest, byteorder="big", signed=True)


def _backoff_seconds(policy: AgentRunRetryPolicy) -> list[int]:
    if not policy.backoff_delays:
        return [0]
    return [max(0, int(delay.total_seconds())) for delay in policy.backoff_delays]


class AgentRepository:
    """Read-only access to the agents table (owned by omni-web)."""

    def __init__(self, pool: Optional[Pool] = None):
        self.pool = pool

    async def _get_pool(self) -> Pool:
        if self.pool:
            return self.pool
        return await get_db_pool()

    async def get_agent(self, agent_id: str) -> Optional[Agent]:
        pool = await self._get_pool()
        query = """
            SELECT id, user_id, name, instructions, agent_type, schedule_type,
                   schedule_value, model_id, allowed_sources, allowed_actions,
                   is_enabled, is_deleted, created_at, updated_at
            FROM agents
            WHERE id = $1 AND NOT is_deleted
        """
        async with pool.acquire() as conn:
            row = await conn.fetchrow(query, agent_id)
        if row:
            return Agent.from_row(dict(row))
        return None

    async def list_agents(self, user_id: str) -> list[Agent]:
        pool = await self._get_pool()
        query = """
            SELECT id, user_id, name, instructions, agent_type, schedule_type,
                   schedule_value, model_id, allowed_sources, allowed_actions,
                   is_enabled, is_deleted, created_at, updated_at
            FROM agents
            WHERE user_id = $1 AND NOT is_deleted
            ORDER BY created_at DESC
        """
        async with pool.acquire() as conn:
            rows = await conn.fetch(query, user_id)
        return [Agent.from_row(dict(r)) for r in rows]

    async def find_due_agents(self, now: datetime) -> list[Agent]:
        """Find enabled agents whose schedule is due and that have no active run."""
        pool = await self._get_pool()
        query = """
            WITH latest_runs AS (
                SELECT DISTINCT ON (agent_id)
                    agent_id,
                    completed_at
                FROM agent_runs
                WHERE status IN ('completed', 'failed')
                ORDER BY agent_id, completed_at DESC NULLS LAST, created_at DESC
            )
            SELECT a.id, a.user_id, a.name, a.instructions, a.agent_type,
                   a.schedule_type, a.schedule_value, a.model_id,
                   a.allowed_sources, a.allowed_actions,
                   a.is_enabled, a.is_deleted, a.created_at, a.updated_at,
                   COALESCE(lr.completed_at, a.created_at) AS last_run_time
            FROM agents a
            LEFT JOIN latest_runs lr ON lr.agent_id = a.id
            WHERE a.is_enabled = TRUE
              AND a.is_deleted = FALSE
              AND NOT EXISTS (
                  SELECT 1
                  FROM agent_runs active
                  WHERE active.agent_id = a.id
                    AND active.status IN ('pending', 'running')
              )
        """
        async with pool.acquire() as conn:
            rows = await conn.fetch(query)

        from .cron_utils import is_due

        due_agents: list[Agent] = []
        for row in rows:
            row_dict = dict(row)
            last_run_time = row_dict.pop("last_run_time")
            agent = Agent.from_row(row_dict)
            try:
                if is_due(agent.schedule_type, agent.schedule_value, last_run_time, now):
                    due_agents.append(agent)
            except Exception as e:
                logger.warning("Failed to compute schedule for agent %s: %s", agent.id, e)

        return due_agents


class AgentRunRepository:
    """Read-write access to the agent_runs durable queue."""

    def __init__(self, pool: Optional[Pool] = None):
        self.pool = pool

    async def _get_pool(self) -> Pool:
        if self.pool:
            return self.pool
        return await get_db_pool()

    async def create_run(
        self,
        agent_id: str,
        trigger_type: AgentRunTriggerType = AgentRunTriggerType.MANUAL,
        max_attempts: int = AGENT_RUN_MAX_ATTEMPTS,
    ) -> AgentRunCreateResult:
        """Create a pending run if the agent is idle; otherwise return the active run."""
        pool = await self._get_pool()
        run_id = str(ULID())
        async with pool.acquire() as conn:
            async with conn.transaction():
                await conn.execute("SELECT pg_advisory_xact_lock($1::bigint)", _advisory_lock_key(agent_id))
                active_row = await conn.fetchrow(
                    f"""
                    SELECT {_AGENT_RUN_COLUMNS}
                    FROM agent_runs
                    WHERE agent_id = $1
                      AND status IN ('pending', 'running')
                    ORDER BY created_at ASC
                    LIMIT 1
                    """,
                    agent_id,
                )
                if active_row:
                    return AgentRunAlreadyActive(AgentRun.from_row(dict(active_row)))

                row = await conn.fetchrow(
                    f"""
                    INSERT INTO agent_runs (
                        id, agent_id, status, trigger_type, max_attempts, created_at
                    )
                    VALUES ($1, $2, 'pending', $3, $4, NOW())
                    RETURNING {_AGENT_RUN_COLUMNS}
                    """,
                    run_id,
                    agent_id,
                    trigger_type.value,
                    max_attempts,
                )
        if row is None:
            raise RuntimeError("agent run insert did not return a row")
        return AgentRun.from_row(dict(row))

    async def update_run(
        self,
        run_id: str,
        status: Optional[RunStatus | str] = None,
        started_at: Optional[datetime] = None,
        completed_at: Optional[datetime] = None,
        summary: Optional[str] = None,
        error_message: Optional[str] = None,
    ) -> Optional[AgentRun]:
        """Legacy narrow updater for non-queue callers. Queue code should not use it."""
        pool = await self._get_pool()

        set_clauses: list[str] = []
        params: list[object] = [run_id]
        idx = 2

        if status is not None:
            status_value = status.value if isinstance(status, RunStatus) else status
            set_clauses.append(f"status = ${idx}")
            params.append(status_value)
            idx += 1
        if started_at is not None:
            set_clauses.append(f"started_at = ${idx}")
            params.append(started_at)
            idx += 1
        if completed_at is not None:
            set_clauses.append(f"completed_at = ${idx}")
            params.append(completed_at)
            idx += 1
        if summary is not None:
            set_clauses.append(f"summary = ${idx}")
            params.append(summary)
            idx += 1
        if error_message is not None:
            set_clauses.append(f"error_message = ${idx}")
            params.append(error_message)
            idx += 1

        if not set_clauses:
            return await self.get_run(run_id)

        query = f"""
            UPDATE agent_runs
            SET {', '.join(set_clauses)}
            WHERE id = $1
            RETURNING {_AGENT_RUN_COLUMNS}
        """
        async with pool.acquire() as conn:
            row = await conn.fetchrow(query, *params)
        if row:
            return AgentRun.from_row(dict(row))
        return None

    async def get_run(self, run_id: str) -> Optional[AgentRun]:
        pool = await self._get_pool()
        query = f"""
            SELECT {_AGENT_RUN_COLUMNS}
            FROM agent_runs
            WHERE id = $1
        """
        async with pool.acquire() as conn:
            row = await conn.fetchrow(query, run_id)
        if row:
            return AgentRun.from_row(dict(row))
        return None

    async def list_runs(
        self, agent_id: str, limit: int = 50, offset: int = 0
    ) -> list[AgentRun]:
        pool = await self._get_pool()
        query = f"""
            SELECT {_AGENT_RUN_COLUMNS}
            FROM agent_runs
            WHERE agent_id = $1
            ORDER BY created_at DESC
            LIMIT $2 OFFSET $3
        """
        async with pool.acquire() as conn:
            rows = await conn.fetch(query, agent_id, limit, offset)
        return [AgentRun.from_row(dict(r)) for r in rows]

    async def get_active_run_for_agent(
        self, agent_id: str, *, include_stale_running: bool = False
    ) -> AgentRun | None:
        pool = await self._get_pool()
        running_clause = (
            "status = 'running'"
            if include_stale_running
            else "(status = 'running' AND lease_expires_at > NOW())"
        )
        query = f"""
            SELECT {_AGENT_RUN_COLUMNS}
            FROM agent_runs
            WHERE agent_id = $1
              AND (status = 'pending' OR {running_clause})
            ORDER BY created_at ASC
            LIMIT 1
        """
        async with pool.acquire() as conn:
            row = await conn.fetchrow(query, agent_id)
        return AgentRun.from_row(dict(row)) if row else None

    async def recover_stale_runs(self) -> int:
        pool = await self._get_pool()
        async with pool.acquire() as conn:
            retry_result = await conn.execute(
                """
                UPDATE agent_runs
                SET status = 'pending',
                    claim_token = NULL,
                    lease_expires_at = NULL,
                    heartbeat_at = NULL
                WHERE status = 'running'
                  AND lease_expires_at <= NOW()
                  AND attempt_count < max_attempts
                """
            )
            failed_result = await conn.execute(
                """
                UPDATE agent_runs
                SET status = 'failed',
                    completed_at = NOW(),
                    claim_token = NULL,
                    lease_expires_at = NULL,
                    heartbeat_at = NULL,
                    error_message = COALESCE(error_message, 'Agent run lease expired and retry attempts were exhausted')
                WHERE status = 'running'
                  AND lease_expires_at <= NOW()
                  AND attempt_count >= max_attempts
                """
            )
        recovered = int(retry_result.split()[-1]) + int(failed_result.split()[-1])
        if recovered:
            logger.info("Recovered %d stale agent run(s)", recovered)
        return recovered

    async def claim_next_run(
        self,
        max_concurrent_runs: int,
        lease_duration: timedelta,
        retry_policy: AgentRunRetryPolicy,
    ) -> AgentRunClaim | None:
        pool = await self._get_pool()
        claim_token = str(ULID())
        lease_seconds = int(lease_duration.total_seconds())
        backoff_seconds = _backoff_seconds(retry_policy)

        async with pool.acquire() as conn:
            async with conn.transaction():
                await conn.execute("SELECT pg_advisory_xact_lock($1::bigint)", _AGENT_RUN_CLAIM_LOCK_KEY)
                active_count = await conn.fetchval(
                    """
                    SELECT COUNT(*)
                    FROM agent_runs
                    WHERE status = 'running'
                      AND lease_expires_at > NOW()
                    """
                )
                if int(active_count or 0) >= max_concurrent_runs:
                    return None

                row = await conn.fetchrow(
                    f"""
                    WITH candidate AS (
                        SELECT r.id
                        FROM agent_runs r
                        WHERE r.status = 'pending'
                          AND r.attempt_count < r.max_attempts
                          AND (
                              r.attempt_count = 0
                              OR r.started_at IS NULL
                              OR r.started_at
                                  + (
                                      INTERVAL '1 second'
                                      * (($3::int[])[LEAST(r.attempt_count, array_length($3::int[], 1))])
                                    ) <= NOW()
                          )
                          AND NOT EXISTS (
                              SELECT 1
                              FROM agent_runs active
                              WHERE active.agent_id = r.agent_id
                                AND active.status = 'running'
                                AND active.lease_expires_at > NOW()
                          )
                        ORDER BY r.created_at ASC
                        LIMIT 1
                        FOR UPDATE SKIP LOCKED
                    )
                    UPDATE agent_runs r
                    SET status = 'running',
                        started_at = NOW(),
                        completed_at = NULL,
                        claim_token = $1,
                        lease_expires_at = NOW() + (INTERVAL '1 second' * $2),
                        heartbeat_at = NOW(),
                        attempt_count = r.attempt_count + 1
                    FROM candidate
                    WHERE r.id = candidate.id
                    RETURNING r.id, r.agent_id, r.status, r.trigger_type, r.started_at,
                              r.completed_at, r.claim_token, r.lease_expires_at,
                              r.heartbeat_at, r.attempt_count, r.max_attempts,
                              r.summary, r.error_message, r.created_at
                    """,
                    claim_token,
                    lease_seconds,
                    backoff_seconds,
                )
        if row is None:
            return None
        return AgentRunClaim(run=AgentRun.from_row(dict(row)), claim_token=claim_token)

    async def heartbeat_run(
        self, run_id: str, claim_token: str, lease_duration: timedelta
    ) -> bool:
        pool = await self._get_pool()
        lease_seconds = int(lease_duration.total_seconds())
        async with pool.acquire() as conn:
            result = await conn.execute(
                """
                UPDATE agent_runs
                SET lease_expires_at = NOW() + (INTERVAL '1 second' * $3),
                    heartbeat_at = NOW()
                WHERE id = $1
                  AND claim_token = $2
                  AND status = 'running'
                """,
                run_id,
                claim_token,
                lease_seconds,
            )
        return int(result.split()[-1]) == 1

    async def list_run_logs(self, run_id: str) -> list[AgentRunLog]:
        pool = await self._get_pool()
        async with pool.acquire() as conn:
            rows = await conn.fetch(
                """
                SELECT id, run_id, message_seq_num, message, created_at
                FROM agent_run_logs
                WHERE run_id = $1
                ORDER BY message_seq_num ASC
                """,
                run_id,
            )
        return [AgentRunLog.from_row(dict(row)) for row in rows]

    async def append_run_log_messages(
        self,
        run_id: str,
        claim_token: str,
        messages: Sequence[AgentRunLogMessage],
    ) -> list[AgentRunLog]:
        if not messages:
            return []

        pool = await self._get_pool()
        async with pool.acquire() as conn:
            async with conn.transaction():
                run_row = await conn.fetchrow(
                    """
                    SELECT id
                    FROM agent_runs
                    WHERE id = $1
                      AND claim_token = $2
                      AND status = 'running'
                    FOR UPDATE
                    """,
                    run_id,
                    claim_token,
                )
                if not run_row:
                    return []

                next_seq = await conn.fetchval(
                    """
                    SELECT COALESCE(MAX(message_seq_num) + 1, 0)
                    FROM agent_run_logs
                    WHERE run_id = $1
                    """,
                    run_id,
                )

                rows: list[AgentRunLog] = []
                for offset, message in enumerate(messages):
                    row = await conn.fetchrow(
                        """
                        INSERT INTO agent_run_logs (id, run_id, message_seq_num, message)
                        VALUES ($1, $2, $3, $4::jsonb)
                        RETURNING id, run_id, message_seq_num, message, created_at
                        """,
                        str(ULID()),
                        run_id,
                        int(next_seq) + offset,
                        json.dumps(message, default=str),
                    )
                    rows.append(AgentRunLog.from_row(dict(row)))
        return rows

    async def complete_run(
        self, run_id: str, claim_token: str, summary: str | None
    ) -> AgentRun | None:
        pool = await self._get_pool()
        async with pool.acquire() as conn:
            row = await conn.fetchrow(
                f"""
                UPDATE agent_runs
                SET status = 'completed',
                    completed_at = NOW(),
                    claim_token = NULL,
                    lease_expires_at = NULL,
                    heartbeat_at = NULL,
                    summary = $3,
                    error_message = NULL
                WHERE id = $1
                  AND claim_token = $2
                  AND status = 'running'
                RETURNING {_AGENT_RUN_COLUMNS}
                """,
                run_id,
                claim_token,
                summary,
            )
        return AgentRun.from_row(dict(row)) if row else None

    async def fail_run(
        self,
        run_id: str,
        claim_token: str,
        error: str,
        retry_policy: AgentRunRetryPolicy,
    ) -> AgentRun | None:
        pool = await self._get_pool()
        async with pool.acquire() as conn:
            row = await conn.fetchrow(
                f"""
                UPDATE agent_runs
                SET status = CASE
                        WHEN attempt_count < LEAST(max_attempts, $3) THEN 'pending'
                        ELSE 'failed'
                    END,
                    completed_at = CASE
                        WHEN attempt_count < LEAST(max_attempts, $3) THEN NULL
                        ELSE NOW()
                    END,
                    claim_token = NULL,
                    lease_expires_at = NULL,
                    heartbeat_at = NULL,
                    error_message = CASE
                        WHEN attempt_count < LEAST(max_attempts, $3) THEN error_message
                        ELSE $4
                    END
                WHERE id = $1
                  AND claim_token = $2
                  AND status = 'running'
                RETURNING {_AGENT_RUN_COLUMNS}
                """,
                run_id,
                claim_token,
                retry_policy.max_attempts,
                error,
            )
        return AgentRun.from_row(dict(row)) if row else None
