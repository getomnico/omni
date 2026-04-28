"""Unit tests for mem0ai role bootstrap.

The bootstrap is run at AI service startup against the privileged omni
DB connection. We patch psycopg and assert the right statements run,
without touching a real Postgres.
"""
from unittest.mock import MagicMock, patch

import pytest


@pytest.mark.unit
class TestRoleBootstrap:
    def _fake_conn(self, role_exists: bool):
        conn = MagicMock()
        cur = MagicMock()
        conn.__enter__ = MagicMock(return_value=conn)
        conn.__exit__ = MagicMock(return_value=False)
        conn.cursor.return_value.__enter__.return_value = cur
        # The impl calls fetchone twice: once for the role-existence probe,
        # then once for the `SELECT format(...)` round-trip that returns the
        # quoted CREATE/ALTER statement.
        existence = (1,) if role_exists else None
        quoted_stmt = (
            ("ALTER ROLE mem0ai PASSWORD 'mem0ai'",)
            if role_exists
            else ("CREATE ROLE mem0ai LOGIN PASSWORD 'mem0ai'",)
        )
        cur.fetchone.side_effect = [existence, quoted_stmt]
        return conn, cur

    def test_creates_role_when_absent(self):
        from memory.role_bootstrap import ensure_mem0ai_role

        conn, cur = self._fake_conn(role_exists=False)
        with patch("memory.role_bootstrap.psycopg.connect", return_value=conn):
            ensure_mem0ai_role(
                dsn="postgresql://omni:pw@db/omni",
                database_name="omni",
                database_username="omni",
                mem0ai_password="mem0ai",
            )

        stmts = " ".join(call.args[0] for call in cur.execute.call_args_list)
        assert "CREATE ROLE mem0ai" in stmts
        assert "GRANT CONNECT ON DATABASE" in stmts
        assert "GRANT USAGE, CREATE ON SCHEMA public TO mem0ai" in stmts
        assert "REVOKE ALL ON ALL TABLES    IN SCHEMA public FROM mem0ai" in stmts
        assert "ALTER DEFAULT PRIVILEGES FOR ROLE" in stmts

    def test_skips_create_role_when_present(self):
        from memory.role_bootstrap import ensure_mem0ai_role

        conn, cur = self._fake_conn(role_exists=True)
        with patch("memory.role_bootstrap.psycopg.connect", return_value=conn):
            ensure_mem0ai_role(
                dsn="postgresql://omni:pw@db/omni",
                database_name="omni",
                database_username="omni",
                mem0ai_password="mem0ai",
            )

        # CREATE ROLE must not be in the executed statements.
        for call in cur.execute.call_args_list:
            assert "CREATE ROLE" not in call.args[0]
        # GRANTs/REVOKEs still run (idempotent).
        stmts = " ".join(call.args[0] for call in cur.execute.call_args_list)
        assert "REVOKE ALL ON ALL TABLES" in stmts

    def test_raises_when_password_missing(self):
        from memory.role_bootstrap import ensure_mem0ai_role

        with pytest.raises(ValueError, match="MEM0AI_DATABASE_ROLE_PASSWORD"):
            ensure_mem0ai_role(
                dsn="postgresql://omni:pw@db/omni",
                database_name="omni",
                database_username="omni",
                mem0ai_password=None,
            )
