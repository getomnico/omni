import asyncpg
from asyncpg import Pool
from typing import Optional
import os
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


async def close_db_pool():
    """Close database connection pool"""
    global _db_pool

    if _db_pool:
        await _db_pool.close()
        _db_pool = None
