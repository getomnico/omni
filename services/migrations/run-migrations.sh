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
