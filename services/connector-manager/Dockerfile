FROM rust:1.82-slim-bookworm AS builder

WORKDIR /app

RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

COPY Cargo.toml Cargo.lock ./
COPY shared ./shared
COPY services/connector-manager ./services/connector-manager

RUN cargo build --release --package omni-connector-manager

FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/omni-connector-manager /usr/local/bin/

ENV RUST_LOG=info
ENV PORT=8090

EXPOSE 8090

CMD ["omni-connector-manager"]
