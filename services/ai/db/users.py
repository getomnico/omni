from typing import Optional
from ulid import ULID
from asyncpg import Pool

from .models import User
from .connection import get_db_pool


class UsersRepository:
    def __init__(self, pool: Optional[Pool] = None):
        self.pool = pool

    async def _get_pool(self) -> Pool:
        if self.pool:
            return self.pool
        return await get_db_pool()

    async def create(
        self,
        email: str,
        password_hash: str,
        full_name: Optional[str] = None,
        role: str = "user",
    ) -> User:
        pool = await self._get_pool()

        user_id = str(ULID())

        query = """
            INSERT INTO users (id, email, password_hash, full_name, role)
            VALUES ($1, $2, $3, $4, $5)
            RETURNING id, email, full_name, role, is_active, created_at, updated_at
        """

        async with pool.acquire() as conn:
            row = await conn.fetchrow(
                query, user_id, email, password_hash, full_name, role
            )

        return User.from_row(dict(row))
