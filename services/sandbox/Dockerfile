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
COPY services/sandbox/ services/sandbox/

RUN if [ "$BUILD_MODE" = "debug" ]; then \
        cargo build --bin omni-sandbox --bin sandbox-exec; \
    else \
        cargo build --release --bin omni-sandbox --bin sandbox-exec; \
    fi

FROM python:3.12-slim-bookworm AS runtime
ARG BUILD_MODE=release

# Install curl for healthcheck and common data processing libraries for user code
RUN apt-get update && \
    apt-get install -y --no-install-recommends curl && \
    rm -rf /var/lib/apt/lists/*

ENV MPLBACKEND=Agg

RUN pip install --no-cache-dir \
    pandas>=2.2.0 \
    numpy>=1.26.0 \
    openpyxl>=3.1.0 \
    matplotlib>=3.9.0 \
    seaborn>=0.13.0

# Run as non-root user
RUN useradd -m sandbox

# Create scratch directory owned by sandbox user
RUN mkdir -p /scratch && chown sandbox:sandbox /scratch

WORKDIR /app
COPY --from=builder /app/target/${BUILD_MODE}/omni-sandbox /usr/local/bin/omni-sandbox
COPY --from=builder /app/target/${BUILD_MODE}/sandbox-exec /usr/local/bin/sandbox-exec

USER sandbox

EXPOSE 8090

CMD ["omni-sandbox"]
