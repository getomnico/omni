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
COPY services/migrations/entrypoint.sh /usr/local/bin/migrator-entrypoint
RUN chmod +x /usr/local/bin/migrator-entrypoint

CMD ["/usr/local/bin/migrator-entrypoint"]
