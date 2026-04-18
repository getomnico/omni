FROM rust:1.91-slim-bookworm AS builder

RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

RUN cargo install sqlx-cli --no-default-features --features native-tls,postgres

FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    jq \
    postgresql-client \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /usr/local/cargo/bin/sqlx /usr/local/bin/sqlx

RUN mkdir /migrations

COPY services/migrations/*.sql ./migrations

CMD bash -c '
ENCODED_PASSWORD=$(printf "%s" "$DATABASE_PASSWORD" | jq -sRr @uri)
SSL_PARAM=$([ "$DATABASE_SSL" = "true" ] && echo "?sslmode=require" || echo "")
DATABASE_URL="postgresql://${DATABASE_USERNAME}:${ENCODED_PASSWORD}@${DATABASE_HOST}:${DATABASE_PORT}/${DATABASE_NAME}${SSL_PARAM}"

# Check if this is an existing deployment that needs the one-time migration
ROLE_EXISTS=$(psql "$DATABASE_URL" -tAc "SELECT 1 FROM pg_roles WHERE rolname = '\''omni_admin'\''")
if [ "$ROLE_EXISTS" != "1" ]; then
  echo "ERROR: This deployment needs a one-time manual migration before running migrations."
  echo ""
  echo "Run this as a superuser in your PostgreSQL database:"
  echo ""
  echo "  CREATE ROLE omni_admin SUPERUSER LOGIN;"
  echo "  ALTER ROLE omni NOSUPERUSER;"
  echo "  ALTER TABLE ALL IN SCHEMA public OWNER TO omni_admin;"
  echo ""
  echo "Then redeploy the migrator. It will create the '\''omni'\'' app role and"
  echo "grant it access to all tables."
  exit 1
fi

sqlx migrate run
'
