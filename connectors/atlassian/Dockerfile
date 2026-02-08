# Multi-stage build for Atlassian connector service
FROM lukemathwalker/cargo-chef:latest-rust-bookworm AS chef
WORKDIR /app

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json

COPY Cargo.toml Cargo.lock ./
COPY shared/ shared/
COPY connectors/atlassian/ connectors/atlassian/
RUN cargo build --release --bin omni-atlassian-connector

FROM debian:bookworm-slim AS runtime
RUN apt-get update && apt-get install -y \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY --from=builder /app/target/release/omni-atlassian-connector /usr/local/bin/omni-atlassian-connector

CMD ["omni-atlassian-connector"]
