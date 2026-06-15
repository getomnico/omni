FROM rust:1.91-slim-bookworm AS builder

RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

RUN cargo install sqlx-cli --version 0.8.6 --locked --no-default-features --features native-tls,postgres

FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    jq \
    postgresql-client \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /usr/local/cargo/bin/sqlx /usr/local/bin/sqlx

RUN mkdir /migrations

COPY services/migrations/*.sql ./migrations/
COPY services/migrations/run-migrations.sh /usr/local/bin/run-migrations

RUN chmod +x /usr/local/bin/run-migrations

CMD ["run-migrations"]
