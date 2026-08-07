import asyncpg
from asyncpg import Connection, Pool
from typing import AsyncIterator, Optional
import os
from contextlib import asynccontextmanager
from urllib.parse import quote_plus

from pgvector.asyncpg import register_vector

_db_pool: Optional[Pool] = None
_db_system_pool: Optional[Pool] = None


def construct_database_url() -> str:
    """Construct database URL from individual components"""
    database_host = os.environ["DATABASE_HOST"]
    database_username = os.environ["DATABASE_USERNAME"]
    database_name = os.environ["DATABASE_NAME"]
    database_password = os.environ["DATABASE_PASSWORD"]
    database_port = os.environ.get("DATABASE_PORT", "5432")

    return f"postgresql://{quote_plus(database_username)}:{quote_plus(database_password)}@{database_host}:{database_port}/{database_name}"


def _construct_database_url_for(
    username_env: str, password_env: str
) -> Optional[str]:
    """Build a URL with alternate credentials when both env vars are present."""
    username = os.environ.get(username_env)
    password = os.environ.get(password_env)
    if not username or not password:
        return None
    database_host = os.environ["DATABASE_HOST"]
    database_name = os.environ["DATABASE_NAME"]
    database_port = os.environ.get("DATABASE_PORT", "5432")
    return f"postgresql://{quote_plus(username)}:{quote_plus(password)}@{database_host}:{database_port}/{database_name}"


async def _init_connection(conn):
    """Initialize connection with pgvector codec."""
    await register_vector(conn)


async def _init_system_connection(conn):
    """Initialize a system-scoped connection with pgvector codec and role."""
    await register_vector(conn)
    await conn.execute("SET ROLE omni_documents_system")


async def get_db_pool() -> Pool:
    """Get or create the user-facing database connection pool."""
    global _db_pool

    if _db_pool is None:
        database_url = construct_database_url()
        _db_pool = await asyncpg.create_pool(
            database_url,
            min_size=5,
            max_size=20,
            max_queries=50000,
            max_inactive_connection_lifetime=300.0,
            command_timeout=60.0,
            init=_init_connection,
        )

    return _db_pool


async def get_system_db_pool() -> Pool:
    """Get or create the privileged document system pool.

    Uses the dedicated system runtime login when configured. Falls back to the
    shared pool (tests, single-credential dev setups) where the connecting role
    can still assume ``omni_documents_system``.
    """
    global _db_system_pool

    if _db_system_pool is None:
        database_url = _construct_database_url_for(
            "DATABASE_SYSTEM_USERNAME", "DATABASE_SYSTEM_PASSWORD"
        )
        if database_url is None:
            return await get_db_pool()
        _db_system_pool = await asyncpg.create_pool(
            database_url,
            min_size=2,
            max_size=10,
            max_queries=50000,
            max_inactive_connection_lifetime=300.0,
            command_timeout=60.0,
            init=_init_system_connection,
        )

    return _db_system_pool


@asynccontextmanager
async def document_user_connection(
    email: str, *, public_only: bool = False, pool: Pool | None = None
) -> AsyncIterator[Connection]:
    db_pool = pool or await get_db_pool()
    async with db_pool.acquire() as conn:
        async with conn.transaction():
            await conn.execute("SET LOCAL ROLE omni_documents_user")
            await conn.execute(
                "SELECT set_config('omni.document_user_email', $1, true)", email
            )
            await conn.execute(
                "SELECT set_config('omni.document_access_scope', $1, true)",
                "public" if public_only else "user",
            )
            yield conn


@asynccontextmanager
async def document_system_connection(
    *, pool: Pool | None = None
) -> AsyncIterator[Connection]:
    db_pool = pool or await get_system_db_pool()
    async with db_pool.acquire() as conn:
        async with conn.transaction():
            yield conn


async def close_db_pool():
    """Close database connection pools."""
    global _db_pool, _db_system_pool

    if _db_system_pool:
        await _db_system_pool.close()
        _db_system_pool = None
    if _db_pool:
        await _db_pool.close()
        _db_pool = None
