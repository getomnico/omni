
from asyncpg import Pool
from ulid import ULID

from .connection import get_db_pool
from .models import User


class UsersRepository:
    def __init__(self, pool: Pool | None = None):
        self.pool = pool

    async def _get_pool(self) -> Pool:
        if self.pool:
            return self.pool
        return await get_db_pool()

    async def create(
        self,
        email: str,
        password_hash: str,
        full_name: str | None = None,
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

    async def find_by_id(self, user_id: str) -> User | None:
        pool = await self._get_pool()
        query = """
            SELECT id, email, full_name, role, is_active, created_at, updated_at
            FROM users WHERE id = $1
        """
        async with pool.acquire() as conn:
            row = await conn.fetchrow(query, user_id)
        if row:
            return User.from_row(dict(row))
        return None
