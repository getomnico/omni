#!/usr/bin/env bash
set -euo pipefail

ENCODED_PASSWORD=$(printf '%s' "$DATABASE_PASSWORD" | jq -sRr @uri)
SSL_PARAM=$([ "${DATABASE_SSL:-false}" = "true" ] && echo "?sslmode=require" || echo "")
export DATABASE_URL="postgresql://${DATABASE_USERNAME}:${ENCODED_PASSWORD}@${DATABASE_HOST}:${DATABASE_PORT}/${DATABASE_NAME}${SSL_PARAM}"

MIGRATIONS_DIR="/migrations"
PARADEDB_023_MIGRATION="$MIGRATIONS_DIR/085_upgrade_paradedb_to_0.23.1.sql"

version_ge() {
    local actual=${1%%-*}
    local minimum=${2%%-*}
    local actual_major actual_minor actual_patch minimum_major minimum_minor minimum_patch

    IFS=. read -r actual_major actual_minor actual_patch <<< "$actual"
    IFS=. read -r minimum_major minimum_minor minimum_patch <<< "$minimum"

    actual_major=${actual_major:-0}
    actual_minor=${actual_minor:-0}
    actual_patch=${actual_patch:-0}
    minimum_major=${minimum_major:-0}
    minimum_minor=${minimum_minor:-0}
    minimum_patch=${minimum_patch:-0}

    if (( actual_major != minimum_major )); then
        (( actual_major > minimum_major ))
        return
    fi
    if (( actual_minor != minimum_minor )); then
        (( actual_minor > minimum_minor ))
        return
    fi
    (( actual_patch >= minimum_patch ))
}

pg_search_version=$(psql "$DATABASE_URL" -Atc "SELECT extversion FROM pg_extension WHERE extname = 'pg_search'" | tr -d '[:space:]')

migration_table_exists=$(psql "$DATABASE_URL" -Atc "SELECT to_regclass('public._sqlx_migrations') IS NOT NULL" | tr -d '[:space:]')
if [[ "$migration_table_exists" == "t" ]]; then
    migration_85_recorded=$(psql "$DATABASE_URL" -Atc "SELECT EXISTS(SELECT 1 FROM public._sqlx_migrations WHERE version = 85)" | tr -d '[:space:]')
else
    migration_85_recorded="f"
fi

# Fresh installs on the ParadeDB 0.24+ image start with pg_search already at
# 0.24+. Historical migration 085 upgrades pg_search from 0.20.6 to 0.23.1 and
# intentionally rejects any other version. We cannot edit that migration because
# it may already be applied in existing deployments and sqlx validates migration
# checksums. For brand-new databases only, baseline migrations through 084, then
# record 085 with its real checksum so sqlx will skip that obsolete downgrade
# path and continue with 086+. Existing deployments that already applied 085, or
# older deployments that still need 085 to upgrade from 0.20.6, continue through
# the normal sqlx path below.
if [[ -n "$pg_search_version" ]] && version_ge "$pg_search_version" "0.24.0" && [[ "$migration_85_recorded" == "f" ]]; then
    sqlx migrate run --source "$MIGRATIONS_DIR" --target-version 84

    checksum=$(sha384sum "$PARADEDB_023_MIGRATION" | awk '{print $1}')
    psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -c "
        INSERT INTO public._sqlx_migrations (version, description, installed_on, success, checksum, execution_time)
        VALUES (85, 'upgrade paradedb to 0.23.1', now(), true, decode('$checksum', 'hex'), 0)
        ON CONFLICT (version) DO NOTHING
    "
fi

sqlx migrate run --source "$MIGRATIONS_DIR"

# Core services must not connect as the migration/table-owner role because owners and
# superusers bypass RLS. User-facing and privileged services use different logins so a
# compromised web/search process cannot assume the system document role.
USER_RUNTIME_USERNAME=${DATABASE_RUNTIME_USERNAME:-omni_runtime}
USER_RUNTIME_PASSWORD=${DATABASE_RUNTIME_PASSWORD:-$DATABASE_PASSWORD}
SYSTEM_RUNTIME_USERNAME=${DATABASE_SYSTEM_USERNAME:-omni_system_runtime}
SYSTEM_RUNTIME_PASSWORD=${DATABASE_SYSTEM_PASSWORD:-$DATABASE_PASSWORD}
psql "$DATABASE_URL" -v ON_ERROR_STOP=1 \
    -v user_runtime_username="$USER_RUNTIME_USERNAME" \
    -v user_runtime_password="$USER_RUNTIME_PASSWORD" \
    -v system_runtime_username="$SYSTEM_RUNTIME_USERNAME" \
    -v system_runtime_password="$SYSTEM_RUNTIME_PASSWORD" <<'SQL'
SELECT format(
    'CREATE ROLE %I LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION PASSWORD %L',
    :'user_runtime_username', :'user_runtime_password'
)
WHERE NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = :'user_runtime_username')
\gexec
SELECT format(
    'ALTER ROLE %I LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS PASSWORD %L',
    :'user_runtime_username', :'user_runtime_password'
)
\gexec
SELECT format('REVOKE omni_documents_system FROM %I', :'user_runtime_username')
\gexec
SELECT format('GRANT omni_documents_user TO %I WITH INHERIT FALSE, SET TRUE', :'user_runtime_username')
\gexec

SELECT format(
    'CREATE ROLE %I LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION PASSWORD %L',
    :'system_runtime_username', :'system_runtime_password'
)
WHERE NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = :'system_runtime_username')
\gexec
SELECT format(
    'ALTER ROLE %I LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS PASSWORD %L',
    :'system_runtime_username', :'system_runtime_password'
)
\gexec
SELECT format('REVOKE omni_documents_user FROM %I', :'system_runtime_username')
\gexec
SELECT format('GRANT omni_documents_system TO %I WITH INHERIT FALSE, SET TRUE', :'system_runtime_username')
\gexec

SELECT format('GRANT USAGE ON SCHEMA public TO %I, %I', :'user_runtime_username', :'system_runtime_username')
\gexec
SELECT format('GRANT ALL PRIVILEGES ON ALL TABLES IN SCHEMA public TO %I, %I', :'user_runtime_username', :'system_runtime_username')
\gexec
SELECT format('GRANT USAGE ON ALL SEQUENCES IN SCHEMA public TO %I, %I', :'user_runtime_username', :'system_runtime_username')
\gexec
SELECT format('REVOKE ALL PRIVILEGES ON documents, embeddings, content_blobs FROM %I, %I', :'user_runtime_username', :'system_runtime_username')
\gexec
SELECT format('ALTER DEFAULT PRIVILEGES IN SCHEMA public GRANT ALL ON TABLES TO %I, %I', :'user_runtime_username', :'system_runtime_username')
\gexec
SELECT format('ALTER DEFAULT PRIVILEGES IN SCHEMA public GRANT USAGE ON SEQUENCES TO %I, %I', :'user_runtime_username', :'system_runtime_username')
\gexec
SQL
