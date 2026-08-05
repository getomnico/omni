"""Repository for group-related database operations."""

from __future__ import annotations

import logging

from asyncpg import Pool

from .connection import get_db_pool

logger = logging.getLogger(__name__)


class GroupRepository:
    """Repository for group membership lookups."""

    def __init__(self, pool: Pool | None = None):
        self.pool = pool

    async def _get_pool(self) -> Pool:
        if self.pool:
            return self.pool
        return await get_db_pool()

    async def find_groups_for_user(self, user_email: str) -> list[str]:
        """Resolve the group emails a user belongs to.

        Returns the group email addresses for all groups the user is a member of.
        Matches the searcher's GroupRepository behavior.
        """
        pool = await self._get_pool()
        rows = await pool.fetch(
            """
            SELECT g.email
            FROM groups g
            JOIN group_memberships gm ON gm.group_id = g.id
            WHERE LOWER(gm.member_email) = LOWER($1)
            """,
            user_email,
        )
        return [row["email"] for row in rows]
