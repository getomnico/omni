# Multi-stage build for Google connector service
FROM lukemathwalker/cargo-chef:latest AS chef
WORKDIR /app

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
ARG BUILD_MODE=release
COPY --from=planner /app/recipe.json recipe.json

# Cook dependencies based on build mode
RUN if [ "$BUILD_MODE" = "debug" ]; then \
        cargo chef cook --recipe-path recipe.json; \
    else \
        cargo chef cook --release --recipe-path recipe.json; \
    fi

COPY . .

# Build binary based on build mode
RUN if [ "$BUILD_MODE" = "debug" ]; then \
        cargo build --bin omni-google-connector; \
    else \
        cargo build --release --bin omni-google-connector; \
    fi

FROM debian:bookworm-slim AS runtime
ARG BUILD_MODE=release
RUN apt-get update && apt-get install -y \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy binary based on build mode
RUN if [ "$BUILD_MODE" = "debug" ]; then \
        echo "Debug mode - binary will be copied from target/debug/"; \
    else \
        echo "Release mode - binary will be copied from target/release/"; \
    fi

COPY --from=builder /app/target/${BUILD_MODE}/omni-google-connector /usr/local/bin/omni-google-connector

EXPOSE 4001
CMD ["omni-google-connector"]
