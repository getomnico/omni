"""Database access for agents (read-only) and agent_runs (read-write)."""

import json
import logging
from datetime import datetime
from typing import Optional

from asyncpg import Pool
from ulid import ULID

from db.connection import get_db_pool, user_db_connection
from .models import Agent, AgentRun

logger = logging.getLogger(__name__)


class AgentRepository:
    """Read-only access to the agents table (owned by omni-web)."""

    def __init__(self, pool: Optional[Pool] = None, user_id: Optional[str] = None):
        self.pool = pool
        self.user_id = user_id

    async def _get_pool(self) -> Pool:
        if self.pool:
            return self.pool
        return await get_db_pool()

    async def get_agent(self, agent_id: str) -> Optional[Agent]:
        if self.user_id:
            query = """
                SELECT id, user_id, name, instructions, agent_type, schedule_type,
                       schedule_value, model_id, allowed_sources, allowed_actions,
                       is_enabled, is_deleted, created_at, updated_at
                FROM agents
                WHERE id = $1 AND NOT is_deleted
            """
            async with user_db_connection(self.user_id) as conn:
                row = await conn.fetchrow(query, agent_id)
        else:
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
        """Find agents that are due for execution.

        Computes next_run_at by looking at the latest completed agent_run
        for each agent (or agents.created_at if no runs exist), then applying
        the schedule. Returns agents where next_run_at <= now.
        """
        pool = await self._get_pool()
        query = """
            WITH latest_runs AS (
                SELECT DISTINCT ON (agent_id)
                    agent_id,
                    completed_at
                FROM agent_runs
                WHERE status IN ('completed', 'failed')
                ORDER BY agent_id, completed_at DESC
            )
            SELECT a.id, a.user_id, a.name, a.instructions, a.agent_type,
                   a.schedule_type, a.schedule_value, a.model_id,
                   a.allowed_sources, a.allowed_actions,
                   a.is_enabled, a.is_deleted, a.created_at, a.updated_at,
                   COALESCE(lr.completed_at, a.created_at) AS last_run_time
            FROM agents a
            LEFT JOIN latest_runs lr ON lr.agent_id = a.id
            LEFT JOIN agent_runs active ON active.agent_id = a.id
                AND active.status IN ('pending', 'running')
            WHERE a.is_enabled = TRUE
              AND a.is_deleted = FALSE
              AND active.id IS NULL
        """
        async with pool.acquire() as conn:
            rows = await conn.fetch(query)

        # Filter in Python using croniter/interval logic
        from .cron_utils import is_due

        due_agents = []
        for row in rows:
            row_dict = dict(row)
            last_run_time = row_dict.pop("last_run_time")
            agent = Agent.from_row(row_dict)
            try:
                if is_due(
                    agent.schedule_type, agent.schedule_value, last_run_time, now
                ):
                    due_agents.append(agent)
            except Exception as e:
                logger.warning(f"Failed to compute schedule for agent {agent.id}: {e}")

        return due_agents


class AgentRunRepository:
    """Read-write access to the agent_runs table (owned by omni-ai)."""

    def __init__(self, pool: Optional[Pool] = None, user_id: Optional[str] = None):
        self.pool = pool
        self.user_id = user_id

    async def _get_pool(self) -> Pool:
        if self.pool:
            return self.pool
        return await get_db_pool()

    async def create_run(self, agent_id: str) -> AgentRun:
        pool = await self._get_pool()
        run_id = str(ULID())
        query = """
            INSERT INTO agent_runs (id, agent_id, status, created_at)
            VALUES ($1, $2, 'pending', NOW())
            RETURNING id, agent_id, status, started_at, completed_at,
                      execution_log, summary, error_message, created_at
        """
        async with pool.acquire() as conn:
            row = await conn.fetchrow(query, run_id, agent_id)
        return AgentRun.from_row(dict(row))

    async def update_run(
        self,
        run_id: str,
        status: Optional[str] = None,
        started_at: Optional[datetime] = None,
        completed_at: Optional[datetime] = None,
        execution_log: Optional[list[dict]] = None,
        summary: Optional[str] = None,
        error_message: Optional[str] = None,
    ) -> Optional[AgentRun]:
        pool = await self._get_pool()

        set_clauses = []
        params = [run_id]
        idx = 2

        if status is not None:
            set_clauses.append(f"status = ${idx}")
            params.append(status)
            idx += 1
        if started_at is not None:
            set_clauses.append(f"started_at = ${idx}")
            params.append(started_at)
            idx += 1
        if completed_at is not None:
            set_clauses.append(f"completed_at = ${idx}")
            params.append(completed_at)
            idx += 1
        if execution_log is not None:
            set_clauses.append(f"execution_log = ${idx}")
            params.append(json.dumps(execution_log, default=str))
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
            RETURNING id, agent_id, status, started_at, completed_at,
                      execution_log, summary, error_message, created_at
        """
        async with pool.acquire() as conn:
            row = await conn.fetchrow(query, *params)
        if row:
            return AgentRun.from_row(dict(row))
        return None

    async def get_run(self, run_id: str) -> Optional[AgentRun]:
        if self.user_id:
            query = """
                SELECT id, agent_id, status, started_at, completed_at,
                       execution_log, summary, error_message, created_at
                FROM agent_runs
                WHERE id = $1
            """
            async with user_db_connection(self.user_id) as conn:
                row = await conn.fetchrow(query, run_id)
        else:
            pool = await self._get_pool()
            query = """
                SELECT id, agent_id, status, started_at, completed_at,
                       execution_log, summary, error_message, created_at
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
        if self.user_id:
            query = """
                SELECT id, agent_id, status, started_at, completed_at,
                       execution_log, summary, error_message, created_at
                FROM agent_runs
                WHERE agent_id = $1
                ORDER BY created_at DESC
                LIMIT $2 OFFSET $3
            """
            async with user_db_connection(self.user_id) as conn:
                rows = await conn.fetch(query, agent_id, limit, offset)
        else:
            pool = await self._get_pool()
            query = """
                SELECT id, agent_id, status, started_at, completed_at,
                       execution_log, summary, error_message, created_at
                FROM agent_runs
                WHERE agent_id = $1
                ORDER BY created_at DESC
                LIMIT $2 OFFSET $3
            """
            async with pool.acquire() as conn:
                rows = await conn.fetch(query, agent_id, limit, offset)
        return [AgentRun.from_row(dict(r)) for r in rows]
