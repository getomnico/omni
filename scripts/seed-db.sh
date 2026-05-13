#!/usr/bin/env bash
set -euo pipefail

# Seed a fresh dev database after stack startup.
# Inserts an admin user, approves the domain, and marks the system as initialized.

PROJECT_NAME="${1:-omni}"
POSTGRES_CONTAINER="${PROJECT_NAME}-postgres"
REDIS_CONTAINER="${PROJECT_NAME}-redis"
MIGRATOR_CONTAINER="${PROJECT_NAME}-migrator"
MAX_WAIT_SECS=120

ADMIN_EMAIL="admin@omni.local"
ADMIN_PASSWORD_HASH='$argon2id$v=19$m=65536,t=3,p=1$C7DiqGmKyt3oSKz/RdKC9w$lyb48nV0cPdfRz2wbPPwV+GBIvbfyL+6JNUDzn0HxrA'
ADMIN_ID='01KRFT782W41QB8Q40XTAP9C3B'
DOMAIN_ID='01KRFT782W41QB8Q40XTAP9C3C'

wait_for_postgres() {
    local elapsed=0
    echo -n "Waiting for ${POSTGRES_CONTAINER}..."
    while ! docker exec "${POSTGRES_CONTAINER}" pg_isready -U omni_dev -d omni_dev >/dev/null 2>&1; do
        if (( elapsed >= MAX_WAIT_SECS )); then
            echo " timeout"
            echo "ERROR: Postgres did not become ready within ${MAX_WAIT_SECS}s" >&2
            exit 1
        fi
        sleep 1
        ((elapsed++))
        echo -n "."
    done
    echo " ready"
}

wait_for_migrator() {
    local elapsed=0
    echo -n "Waiting for ${MIGRATOR_CONTAINER}..."
    while true; do
        local status
        status="$(docker inspect -f '{{.State.Status}}' "${MIGRATOR_CONTAINER}" 2>/dev/null || echo 'unknown')"
        if [[ "${status}" == "exited" ]]; then
            local exit_code
            exit_code="$(docker inspect -f '{{.State.ExitCode}}' "${MIGRATOR_CONTAINER}" 2>/dev/null || echo 1)"
            if [[ "${exit_code}" == "0" ]]; then
                echo " completed"
                return 0
            else
                echo " failed"
                echo "ERROR: Migrator exited with code ${exit_code}" >&2
                docker logs "${MIGRATOR_CONTAINER}" >&2 || true
                exit 1
            fi
        fi
        if (( elapsed >= MAX_WAIT_SECS )); then
            echo " timeout"
            echo "ERROR: Migrator did not complete within ${MAX_WAIT_SECS}s" >&2
            exit 1
        fi
        sleep 1
        ((elapsed++))
        echo -n "."
    done
}

seed_user() {
    echo "Seeding admin user..."

    local existing
    existing="$(docker exec "${POSTGRES_CONTAINER}" psql -U omni_dev -d omni_dev -Atc "SELECT COUNT(*) FROM users WHERE email = '${ADMIN_EMAIL}';" 2>/dev/null || echo '0')"

    if [[ "${existing}" != "0" ]]; then
        echo "  Admin user already exists, skipping."
        return 0
    fi

    docker exec "${POSTGRES_CONTAINER}" psql -U omni_dev -d omni_dev -q -c "
        INSERT INTO users (id, email, password_hash, full_name, role, is_active, auth_method, domain, created_at, updated_at)
        VALUES ('${ADMIN_ID}', '${ADMIN_EMAIL}', '${ADMIN_PASSWORD_HASH}', 'Admin User', 'admin', true, 'password', 'omni.local', NOW(), NOW());
    "

    docker exec "${POSTGRES_CONTAINER}" psql -U omni_dev -d omni_dev -q -c "
        INSERT INTO approved_domains (id, domain, approved_by, created_at, updated_at)
        VALUES ('${DOMAIN_ID}', 'omni.local', '${ADMIN_ID}', NOW(), NOW());
    "

    echo "  Admin user created: ${ADMIN_EMAIL}"
}

seed_redis() {
    echo "Marking system as initialized in Redis..."
    docker exec "${REDIS_CONTAINER}" redis-cli HSET system:flags initialized true >/dev/null 2>&1 || true
}

echo "=== Database seed for ${PROJECT_NAME} ==="
wait_for_postgres
wait_for_migrator
seed_user
seed_redis

echo ""
echo "Done. Log in at http://localhost:3000/login"
echo "  Email:    ${ADMIN_EMAIL}"
echo "  Password: omni-dev"
echo ""
