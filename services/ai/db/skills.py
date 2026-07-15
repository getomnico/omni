"""Read-only repository for user-created library skills.

Only the omni-web service writes to the skills table. This repository
provides read-only access for the AI service to discover skills visible
to a given user (public skills + the user's own private skills).
"""

from __future__ import annotations

import logging
from dataclasses import dataclass
from datetime import datetime
from typing import Literal

from asyncpg import Pool

from .connection import get_db_pool

logger = logging.getLogger(__name__)


@dataclass
class Skill:
    id: str
    owner_id: str
    name: str
    instructions: str
    visibility: Literal["private", "public"]
    created_at: datetime
    updated_at: datetime


class SkillsRepository:
    """Read-only access to user-created library skills."""

    def __init__(self, pool: Pool | None = None):
        self.pool = pool

    async def _get_pool(self) -> Pool:
        if self.pool:
            return self.pool
        return await get_db_pool()

    async def list_visible(self, user_id: str) -> list[Skill]:
        """Return skills visible to the given user: public + user's own private."""
        pool = await self._get_pool()
        rows = await pool.fetch(
            """
            SELECT id, owner_id, name, instructions, visibility,
                   created_at, updated_at
            FROM skills
            WHERE owner_id = $1 OR visibility = 'public'
            ORDER BY updated_at DESC
            """,
            user_id,
        )
        return [Skill(**dict(row)) for row in rows]

    async def get_visible_by_id(self, skill_id: str, user_id: str) -> Skill | None:
        """Get a single skill if visible to the given user."""
        pool = await self._get_pool()
        row = await pool.fetchrow(
            """
            SELECT id, owner_id, name, instructions, visibility,
                   created_at, updated_at
            FROM skills
            WHERE id = $1
              AND (owner_id = $2 OR visibility = 'public')
            """,
            skill_id,
            user_id,
        )
        return Skill(**dict(row)) if row else None
