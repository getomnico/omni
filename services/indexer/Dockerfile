# Multi-stage build for Indexer service
FROM lukemathwalker/cargo-chef:latest-rust-1.90.0-bookworm  AS chef
WORKDIR /app

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json

COPY Cargo.toml Cargo.lock ./
COPY shared/ shared/
COPY services/indexer/ services/indexer/
RUN cargo build --release --bin omni-indexer

FROM debian:bookworm-slim AS runtime
RUN apt-get update && apt-get install -y \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY --from=builder /app/target/release/omni-indexer /usr/local/bin/omni-indexer

CMD ["omni-indexer"]
