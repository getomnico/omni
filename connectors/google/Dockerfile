# Multi-stage build for Google connector service
FROM lukemathwalker/cargo-chef:latest AS chef
WORKDIR /app

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json

COPY . .
RUN cargo build --release --bin clio-google-connector

FROM debian:bookworm-slim AS runtime
RUN apt-get update && apt-get install -y \
    ca-certificates \
    wget \
    && rm -rf /var/lib/apt/lists/*

# Download and install pdfium binary
RUN wget https://github.com/bblanchon/pdfium-binaries/releases/download/chromium/6721/pdfium-linux-x64.tgz && \
    tar -xzf pdfium-linux-x64.tgz && \
    mv lib/libpdfium.so /usr/local/lib/ && \
    ldconfig && \
    rm -rf pdfium-linux-x64.tgz lib include

WORKDIR /app
COPY --from=builder /app/target/release/clio-google-connector /usr/local/bin/clio-google-connector

EXPOSE 4001
CMD ["clio-google-connector"]