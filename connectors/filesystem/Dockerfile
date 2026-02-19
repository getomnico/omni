FROM lukemathwalker/cargo-chef:latest-rust-1.91.0-bookworm AS chef
WORKDIR /app

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
ARG BUILD_MODE=release
COPY --from=planner /app/recipe.json recipe.json

RUN if [ "$BUILD_MODE" = "debug" ]; then \
        cargo chef cook --recipe-path recipe.json; \
    else \
        cargo chef cook --release --recipe-path recipe.json; \
    fi

COPY Cargo.toml Cargo.lock ./
COPY shared/ shared/
COPY connectors/filesystem/ connectors/filesystem/

RUN if [ "$BUILD_MODE" = "debug" ]; then \
        cargo build --bin omni-filesystem-connector; \
    else \
        cargo build --release --bin omni-filesystem-connector; \
    fi

FROM debian:bookworm-slim AS runtime
ARG BUILD_MODE=release
RUN apt-get update && apt-get install -y \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=builder /app/target/${BUILD_MODE}/omni-filesystem-connector /usr/local/bin/omni-filesystem-connector

CMD ["omni-filesystem-connector"]
