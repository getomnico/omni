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
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /usr/local/cargo/bin/sqlx /usr/local/bin/sqlx

RUN mkdir /migrations

COPY services/migrations/*.sql ./migrations

CMD ["bash", "-c", "ENCODED_PASSWORD=$(printf '%s' \"$DATABASE_PASSWORD\" | jq -sRr @uri) && SSL_PARAM=$([ \"$DATABASE_SSL\" = \"true\" ] && echo \"?sslmode=require\" || echo \"\") && export DATABASE_URL=\"postgresql://${DATABASE_USERNAME}:${ENCODED_PASSWORD}@${DATABASE_HOST}:${DATABASE_PORT}/${DATABASE_NAME}${SSL_PARAM}\" && sqlx migrate run"]
