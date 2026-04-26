#!/usr/bin/env bash
set -euo pipefail

ENCODED_PASSWORD=$(printf '%s' "$DATABASE_PASSWORD" | jq -sRr @uri)
SSL_PARAM=$([ "${DATABASE_SSL:-false}" = "true" ] && echo "?sslmode=require" || echo "")
export DATABASE_URL="postgresql://${DATABASE_USERNAME}:${ENCODED_PASSWORD}@${DATABASE_HOST}:${DATABASE_PORT}/${DATABASE_NAME}${SSL_PARAM}"

ROLE_EXISTS=$(psql "$DATABASE_URL" -tAc "SELECT 1 FROM pg_roles WHERE rolname = 'omni_admin'")
if [ "$ROLE_EXISTS" != "1" ]; then
  echo "ERROR: This deployment needs a one-time manual migration before running migrations."
  echo ""
  echo "Run this as a superuser in your PostgreSQL database:"
  echo ""
  echo "  CREATE ROLE omni_admin SUPERUSER LOGIN;"
  echo "  ALTER ROLE omni NOSUPERUSER;"
  echo "  ALTER TABLE ALL IN SCHEMA public OWNER TO omni_admin;"
  echo ""
  echo "Then redeploy the migrator. It will create the 'omni' app role and"
  echo "grant it access to all tables."
  exit 1
fi

exec sqlx migrate run
