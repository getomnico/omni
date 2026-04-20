# GitHub Connector Dockerfile

# Stage 1: Build the official GitHub MCP server (Go binary)
FROM golang:1.25-alpine AS mcp-builder

RUN apk add --no-cache git
RUN go install github.com/github/github-mcp-server/cmd/github-mcp-server@v1.0.0

# Stage 2: Build Python wheels
FROM python:3.11-slim AS builder

WORKDIR /build

# Install build dependencies
RUN pip install --no-cache-dir hatchling

# Copy and build the SDK
COPY sdk/python /sdk/python
RUN pip wheel --no-deps -w /wheels /sdk/python

# Copy and build the connector
COPY connectors/github /build
RUN pip wheel --no-deps -w /wheels /build

# Stage 3: Production
FROM python:3.11-slim

WORKDIR /app

# Install runtime dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Copy GitHub MCP server binary
COPY --from=mcp-builder /go/bin/github-mcp-server /usr/local/bin/

# Copy wheels and install
COPY --from=builder /wheels /wheels
RUN pip install --no-cache-dir /wheels/*.whl && rm -rf /wheels

# Copy entry point
COPY connectors/github/main.py /app/

ENV PYTHONUNBUFFERED=1

CMD ["python", "main.py"]
