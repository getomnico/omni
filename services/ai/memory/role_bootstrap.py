"""Ensure the restricted `mem0ai` Postgres role exists with correct grants.

Runs at AI-service startup against the privileged omni DB connection
(the one the application itself uses). Idempotent: safe to call on
every boot — creates the role only if missing, re-applies the grants
and revokes unconditionally so policy drift is self-healing.
"""
import logging
import re

import psycopg

logger = logging.getLogger(__name__)

# Plain unquoted Postgres identifier — safe to inline into DO blocks.
_ROLE_NAME_RE = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*$")


def ensure_mem0ai_role(
    dsn: str,
    database_name: str,
    database_username: str,
    mem0ai_password: str | None,
    role_name: str = "mem0ai",
) -> None:
    """Create the role if missing and (re)apply its grants/revokes.

    Args:
        dsn: Privileged connection string (the main omni role).
        database_name: Main omni DB name — used in `GRANT CONNECT`.
        database_username: Owner of public tables — used in
            `ALTER DEFAULT PRIVILEGES FOR ROLE` so future omni migrations
            do not silently grant access to the role.
        mem0ai_password: Plaintext password for the role login.
        role_name: Postgres role name. Defaults to ``mem0ai``.

    Raises:
        ValueError: if `mem0ai_password` is missing or `role_name` is not
            a plain identifier.
    """
    if not mem0ai_password:
        raise ValueError(
            "MEM0AI_DATABASE_ROLE_PASSWORD is required when MEMORY_ENABLED=true"
        )
    if not _ROLE_NAME_RE.match(role_name):
        raise ValueError(f"Invalid mem0ai role name: {role_name!r}")

    r = role_name

    with psycopg.connect(dsn, autocommit=True) as conn, conn.cursor() as cur:
        cur.execute("SELECT 1 FROM pg_roles WHERE rolname = %s", (r,))
        exists = cur.fetchone() is not None

        if not exists:
            cur.execute(
                f"SELECT format('CREATE ROLE {r} LOGIN PASSWORD %%L', %s::text)",
                (mem0ai_password,),
            )
            cur.execute(cur.fetchone()[0])
            logger.info(f"Created {r} role")
        else:
            # Rotate the password on every startup so the env var is the
            # source of truth — operators can change it without manual SQL.
            cur.execute(
                f"SELECT format('ALTER ROLE {r} PASSWORD %%L', %s::text)",
                (mem0ai_password,),
            )
            cur.execute(cur.fetchone()[0])

        # Connect and basic schema access.
        cur.execute(f'GRANT CONNECT ON DATABASE "{database_name}" TO {r}')
        cur.execute(f"GRANT USAGE, CREATE ON SCHEMA public TO {r}")

        # Reassign mem0 tables in case a prior boot created them under another role.
        cur.execute(
            f"DO $$ DECLARE rec RECORD; BEGIN "
            f"FOR rec IN SELECT tablename FROM pg_tables "
            f"WHERE schemaname='public' AND tablename LIKE 'mem0%' "
            f"AND tableowner <> '{r}' LOOP "
            f"EXECUTE format('ALTER TABLE public.%I OWNER TO {r}', rec.tablename); "
            f"END LOOP; END $$;"
        )

        # Strip grants on existing omni tables.
        cur.execute(f"REVOKE ALL ON ALL TABLES    IN SCHEMA public FROM {r}")
        cur.execute(f"REVOKE ALL ON ALL SEQUENCES IN SCHEMA public FROM {r}")
        cur.execute(f"REVOKE ALL ON ALL FUNCTIONS IN SCHEMA public FROM {r}")

        # Restore mem0's access to its own tables (blanket REVOKE above strips it).
        cur.execute(
            f"DO $$ DECLARE rec RECORD; BEGIN "
            f"FOR rec IN SELECT tablename FROM pg_tables "
            f"WHERE schemaname='public' AND tablename LIKE 'mem0%' LOOP "
            f"EXECUTE format('GRANT ALL ON TABLE public.%I TO {r}', rec.tablename); "
            f"END LOOP; END $$;"
        )
        cur.execute(
            f"DO $$ DECLARE rec RECORD; BEGIN "
            f"FOR rec IN SELECT c.relname FROM pg_class c "
            f"JOIN pg_namespace n ON n.oid = c.relnamespace "
            f"WHERE n.nspname='public' AND c.relkind='S' "
            f"AND c.relname LIKE 'mem0%' LOOP "
            f"EXECUTE format('GRANT ALL ON SEQUENCE public.%I TO {r}', rec.relname); "
            f"END LOOP; END $$;"
        )

        # Block future omni-owned tables from auto-granting to mem0.
        cur.execute(
            f'ALTER DEFAULT PRIVILEGES FOR ROLE "{database_username}" '
            f"IN SCHEMA public REVOKE ALL ON TABLES    FROM {r}"
        )
        cur.execute(
            f'ALTER DEFAULT PRIVILEGES FOR ROLE "{database_username}" '
            f"IN SCHEMA public REVOKE ALL ON SEQUENCES FROM {r}"
        )

    logger.info(f"{r} role grants and revokes applied")
