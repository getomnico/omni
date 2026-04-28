import asyncpg
from asyncpg import Pool, Connection
from typing import Optional, AsyncIterator
import os
from contextlib import asynccontextmanager
from urllib.parse import quote_plus

from pgvector.asyncpg import register_vector

_db_pool: Optional[Pool] = None


def construct_database_url() -> str:
    """Construct database URL from individual components"""
    database_host = os.environ["DATABASE_HOST"]
    database_username = os.environ["DATABASE_USERNAME"]
    database_name = os.environ["DATABASE_NAME"]
    database_password = os.environ["DATABASE_PASSWORD"]
    database_port = os.environ.get("DATABASE_PORT", "5432")

    return f"postgresql://{quote_plus(database_username)}:{quote_plus(database_password)}@{database_host}:{database_port}/{database_name}"


async def _init_connection(conn):
    """Initialize connection with pgvector codec."""
    await register_vector(conn)


async def get_db_pool() -> Pool:
    """Get or create database connection pool"""
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


@asynccontextmanager
async def user_db_connection(user_id: str) -> AsyncIterator[Connection]:
    """Context manager that provides a DB connection with RLS context set.

    Acquires a connection from the pool, sets app.current_user_id for RLS,
    and releases it back when done.
    """
    pool = await get_db_pool()
    conn = await pool.acquire()
    try:
        await conn.execute("SET app.current_user_id = $1", user_id)
        yield conn
    finally:
        await pool.release(conn)


@asynccontextmanager
async def system_db_connection() -> AsyncIterator[Connection]:
    """Context manager for system-level queries that bypass per-user RLS.

    Sets `app.is_admin = 'true'`, which RLS policies recognise as the
    indexer / background-worker context (cross-user dedup, embedding
    status updates, etc.). Use sparingly — only for code paths that
    legitimately need to operate across all users.
    """
    pool = await get_db_pool()
    conn = await pool.acquire()
    try:
        await conn.execute("SET app.is_admin = 'true'")
        yield conn
    finally:
        await pool.release(conn)


async def close_db_pool():
    """Close database connection pool"""
    global _db_pool

    if _db_pool:
        await _db_pool.close()
        _db_pool = None
