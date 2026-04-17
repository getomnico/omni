-- ============================================================
-- 083: Create dedicated app role for RLS
-- ============================================================
-- In PostgreSQL, the table owner bypasses RLS policies.
-- This migration creates the 'omni' app role and grants it
-- access to all tables. The app connects as 'omni' so RLS
-- policies are enforced.
--
-- The migration user (superuser) retains table ownership and
-- bypasses RLS — only the 'omni' app role is subject to policies.
--
-- Backward compatibility:
-- - Existing deployments: tables already owned by the current user.
--   This migration just grants 'omni' access. The app must switch
--   DATABASE_USERNAME from the current user to 'omni'.
-- - New deployments: migrator runs as superuser, owns all tables,
--   app connects as 'omni'. RLS is enforced from the start.
-- ============================================================

DO $$
BEGIN
  IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'omni') THEN
    CREATE ROLE omni;
  END IF;
END
$$;

-- Grant CONNECT on the current database
DO $$
BEGIN
  EXECUTE format('GRANT CONNECT ON DATABASE %I TO omni', current_database());
END
$$;

-- Grant USAGE on public schema
GRANT USAGE ON SCHEMA public TO omni;

-- Grant all privileges on all existing tables
GRANT ALL PRIVILEGES ON ALL TABLES IN SCHEMA public TO omni;

-- Grant usage on all sequences
GRANT USAGE ON ALL SEQUENCES IN SCHEMA public TO omni;

-- Grant future privileges (for tables created later by the migration user)
ALTER DEFAULT PRIVILEGES IN SCHEMA public GRANT ALL ON TABLES TO omni;
ALTER DEFAULT PRIVILEGES IN SCHEMA public GRANT ALL ON SEQUENCES TO omni;
