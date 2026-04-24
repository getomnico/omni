FROM lukemathwalker/cargo-chef:latest-rust-1.91.1-bookworm AS chef
WORKDIR /app

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json

COPY Cargo.toml Cargo.lock ./
COPY shared/ shared/
COPY sdk/rust/ sdk/rust/
COPY connectors/fireflies/ connectors/fireflies/
RUN cargo build --release --bin omni-fireflies-connector

FROM debian:bookworm-slim AS runtime
RUN apt-get update && apt-get install -y \
    ca-certificates \
    curl \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY --from=builder /app/target/release/omni-fireflies-connector /usr/local/bin/omni-fireflies-connector

CMD ["omni-fireflies-connector"]
