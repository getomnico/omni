-- ============================================================
-- 083: Create dedicated roles for RLS
-- ============================================================
-- PostgreSQL: the table owner bypasses RLS. This migration:
--   1. Creates 'omni_admin' (superuser) — owns all tables
--   2. Creates/demotes 'omni' (non-superuser, no ownership)
--   3. Grants 'omni' access to all tables
--
-- Existing deployments: 'omni' already exists as superuser + owner.
--   → Ownership transfers to 'omni_admin', 'omni' is demoted.
-- New deployments: 'omni_admin' is the superuser, 'omni' is
--   a plain role. RLS is enforced from day one.
--
-- App services connect as 'omni'. The migrator connects as
-- 'omni_admin'.
-- ============================================================

-- 1. Create omni_admin role (superuser)
DO $$
BEGIN
  IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'omni_admin') THEN
    CREATE ROLE omni_admin SUPERUSER LOGIN;
  END IF;
END
$$;

-- 2. Transfer ownership of all tables to omni_admin
DO $$
DECLARE
    r RECORD;
BEGIN
    FOR r IN (
        SELECT tablename, tableowner
        FROM pg_tables
        WHERE schemaname = 'public'
          AND tableowner != 'omni_admin'
    ) LOOP
        EXECUTE format('ALTER TABLE %I OWNER TO omni_admin', r.tablename);
    END LOOP;
END
$$;

-- 3. Create 'omni' role (non-superuser, with LOGIN)
DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'omni') THEN
        CREATE ROLE omni LOGIN;
    ELSE
        -- Existing deployment: revoke superuser if granted
        ALTER ROLE omni NOSUPERUSER;
    END IF;
END
$$;

-- 4. Grant CONNECT on the current database
DO $$
BEGIN
    EXECUTE format('GRANT CONNECT ON DATABASE %I TO omni', current_database());
END
$$;

-- 5. Grant USAGE on public schema
GRANT USAGE ON SCHEMA public TO omni;

-- 6. Grant all privileges on all existing tables
GRANT ALL PRIVILEGES ON ALL TABLES IN SCHEMA public TO omni;

-- 7. Grant usage on all sequences
GRANT ALL PRIVILEGES ON ALL SEQUENCES IN SCHEMA public TO omni;

-- 8. Set default privileges so future tables are accessible to 'omni'
ALTER DEFAULT PRIVILEGES IN SCHEMA public GRANT ALL ON TABLES TO omni;
ALTER DEFAULT PRIVILEGES IN SCHEMA public GRANT ALL ON SEQUENCES TO omni;
