
from asyncpg import Pool
from ulid import ULID

from .connection import get_db_pool
from .models import User, UserConfiguration


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
        user_query = """
            SELECT id, email, full_name, role, is_active, created_at, updated_at
            FROM users
            WHERE id = $1
        """
        configuration_query = """
            SELECT key, value
            FROM configuration
            WHERE scope = 'user' AND user_id = $1
        """
        async with pool.acquire() as conn:
            row = await conn.fetchrow(user_query, user_id)
            if not row:
                return None
            configuration_rows = await conn.fetch(configuration_query, user_id)

        user_row = dict(row)
        user_row["configuration"] = UserConfiguration.from_rows(
            [dict(config_row) for config_row in configuration_rows]
        )
        return User.from_row(user_row)
