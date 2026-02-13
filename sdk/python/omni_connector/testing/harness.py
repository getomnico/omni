"""OmniTestHarness — manages testcontainers for integration tests."""

from __future__ import annotations

import asyncio
import logging
import os
import pathlib
import time

import asyncpg
from testcontainers.core.container import DockerContainer
from testcontainers.core.waiting_utils import wait_for_logs
from testcontainers.redis import RedisContainer

from .seed import SeedHelper

logger = logging.getLogger(__name__)

PARADEDB_IMAGE = "paradedb/paradedb:0.20.6-pg17"
POSTGRES_USER = "omni"
POSTGRES_PASSWORD = "omni_password"
POSTGRES_DB = "omni_test"

ENCRYPTION_KEY = "test_master_key_that_is_long_enough_32_chars"
ENCRYPTION_SALT = "test_salt_16_chars"


def _find_project_root() -> pathlib.Path:
    """Walk up from this file to find the repo root (contains Cargo.toml)."""
    current = pathlib.Path(__file__).resolve()
    for parent in [current] + list(current.parents):
        if (parent / "Cargo.toml").exists():
            return parent
    raise RuntimeError("Cannot find project root (no Cargo.toml found)")


class OmniTestHarness:
    """Manages ParadeDB, Redis, and connector-manager containers for integration tests.

    Two-phase startup:
      1. start_infra() — ParadeDB + Redis, runs SQL migrations
      2. start_connector_manager(connector_env_vars) — connector-manager container
    """

    def __init__(self) -> None:
        self._project_root = _find_project_root()
        self._pg_container: DockerContainer | None = None
        self._redis_container: RedisContainer | None = None
        self._cm_container: DockerContainer | None = None
        self._db_pool: asyncpg.Pool | None = None
        self._pg_port: int = 0
        self._redis_port: int = 0
        self._cm_port: int = 0

    @property
    def db_pool(self) -> asyncpg.Pool:
        assert self._db_pool is not None, "Call start_infra() first"
        return self._db_pool

    @property
    def connector_manager_url(self) -> str:
        assert self._cm_port, "Call start_connector_manager() first"
        return f"http://localhost:{self._cm_port}"

    @property
    def redis_url(self) -> str:
        assert self._redis_port, "Call start_infra() first"
        return f"redis://localhost:{self._redis_port}"

    def seed(self) -> SeedHelper:
        return SeedHelper(self.db_pool)

    async def start_infra(self) -> None:
        """Start ParadeDB + Redis, run migrations."""
        self._pg_container = (
            DockerContainer(PARADEDB_IMAGE)
            .with_exposed_ports(5432)
            .with_env("POSTGRES_DB", POSTGRES_DB)
            .with_env("POSTGRES_USER", POSTGRES_USER)
            .with_env("POSTGRES_PASSWORD", POSTGRES_PASSWORD)
        )
        self._pg_container.start()
        wait_for_logs(
            self._pg_container,
            "database system is ready to accept connections",
            timeout=60,
        )
        # ParadeDB logs the "ready" message twice (once during init, once after restart)
        time.sleep(5)
        self._pg_port = int(self._pg_container.get_exposed_port(5432))

        self._redis_container = RedisContainer()
        self._redis_container.start()
        self._redis_port = int(self._redis_container.get_exposed_port(6379))

        dsn = (
            f"postgresql://{POSTGRES_USER}:{POSTGRES_PASSWORD}"
            f"@localhost:{self._pg_port}/{POSTGRES_DB}"
        )
        self._db_pool = await asyncpg.create_pool(
            dsn, min_size=2, max_size=10, ssl="disable"
        )

        await self._run_migrations()
        await self._drop_encryption_constraint()

    async def start_connector_manager(
        self, connector_env_vars: dict[str, str] | None = None
    ) -> None:
        """Build and start the connector-manager Docker container."""
        pg_host = self._host_accessible_address()

        env = {
            "DATABASE_HOST": pg_host,
            "DATABASE_PORT": str(self._pg_port),
            "DATABASE_USERNAME": POSTGRES_USER,
            "DATABASE_PASSWORD": POSTGRES_PASSWORD,
            "DATABASE_NAME": POSTGRES_DB,
            "REDIS_URL": f"redis://{pg_host}:{self._redis_port}",
            "PORT": "8090",
            "ENCRYPTION_KEY": ENCRYPTION_KEY,
            "ENCRYPTION_SALT": ENCRYPTION_SALT,
            "SCHEDULER_INTERVAL_SECONDS": "3600",
            "STALE_SYNC_TIMEOUT_MINUTES": "60",
            "MAX_CONCURRENT_SYNCS": "10",
            "MAX_CONCURRENT_SYNCS_PER_TYPE": "5",
        }
        if connector_env_vars:
            env.update(connector_env_vars)

        dockerfile_context = str(self._project_root)

        image_tag = self._build_connector_manager_image(dockerfile_context)

        self._cm_container = DockerContainer(image_tag).with_exposed_ports(8090)
        for k, v in env.items():
            self._cm_container = self._cm_container.with_env(k, v)

        # Allow container to reach host services (ParadeDB, Redis, connector)
        self._cm_container.with_kwargs(
            extra_hosts={"host.docker.internal": "host-gateway"}
        )

        self._cm_container.start()
        self._cm_port = int(self._cm_container.get_exposed_port(8090))

        self._wait_for_cm_healthy()

    def _build_connector_manager_image(self, context: str) -> str:
        """Build the connector-manager Docker image and return the tag."""
        import docker

        tag = "omni-connector-manager:test"
        client = docker.from_env()

        logger.info(
            "Building connector-manager image (this may take a while on first run)..."
        )
        client.images.build(
            path=context,
            dockerfile="services/connector-manager/Dockerfile",
            tag=tag,
            buildargs={"BUILD_MODE": "release"},
            rm=True,
        )
        logger.info("Connector-manager image built successfully")
        return tag

    def _wait_for_cm_healthy(self, timeout: float = 60) -> None:
        """Poll the connector-manager /health endpoint until it responds."""
        import httpx

        url = f"{self.connector_manager_url}/health"
        deadline = time.monotonic() + timeout
        last_err = None
        while time.monotonic() < deadline:
            try:
                resp = httpx.get(url, timeout=2)
                if resp.status_code == 200:
                    logger.info("Connector-manager is healthy")
                    return
            except Exception as e:
                last_err = e
            time.sleep(0.5)
        raise TimeoutError(
            f"Connector-manager did not become healthy within {timeout}s: {last_err}"
        )

    def _host_accessible_address(self) -> str:
        """Return the hostname containers can use to reach host services."""
        return "host.docker.internal"

    async def _run_migrations(self) -> None:
        """Execute all SQL migration files in sorted order."""
        migration_dir = self._project_root / "services" / "migrations"
        if not migration_dir.exists():
            raise RuntimeError(f"Migration directory not found: {migration_dir}")

        sql_files = sorted(migration_dir.glob("*.sql"))
        logger.info("Running %d migrations...", len(sql_files))

        for sql_file in sql_files:
            sql = sql_file.read_text()
            try:
                await self._db_pool.execute(sql)
            except Exception as e:
                logger.warning("Migration %s had an issue: %s", sql_file.name, e)

    async def _drop_encryption_constraint(self) -> None:
        """Drop the encryption CHECK constraint so we can store plain credentials in tests."""
        await self._db_pool.execute(
            "ALTER TABLE service_credentials "
            "DROP CONSTRAINT IF EXISTS service_credentials_encrypted_check"
        )

    async def teardown(self) -> None:
        """Stop all containers and close connections."""
        if self._db_pool:
            await self._db_pool.close()
        if self._cm_container:
            self._cm_container.stop()
        if self._redis_container:
            self._redis_container.stop()
        if self._pg_container:
            self._pg_container.stop()
