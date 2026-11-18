# Build stage
FROM python:3.12-slim AS builder

ENV PYTHONDONTWRITEBYTECODE=1
ENV PYTHONUNBUFFERED=1
ENV UV_COMPILE_BYTECODE=1
ENV UV_LINK_MODE=copy
ENV UV_PROJECT_ENVIRONMENT=/usr/local

# Install build dependencies
RUN apt-get update && apt-get install -y \
    gcc \
    g++ \
    curl \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Install uv
ADD https://astral.sh/uv/0.10.9/install.sh /uv-installer.sh
RUN sh /uv-installer.sh && rm /uv-installer.sh && mv /root/.local/bin/uv /bin/uv && mv /root/.local/bin/uvx /bin/uvx

WORKDIR /app

# Copy dependency files
COPY pyproject.toml uv.lock ./

# Install dependencies directly to system Python (cached layer)
RUN uv sync --locked --no-dev --no-install-project

# Copy source and install project
COPY . .
RUN uv sync --locked --no-dev --no-editable

# Runtime stage
FROM python:3.12-slim AS runtime

ENV PYTHONDONTWRITEBYTECODE=1
ENV PYTHONUNBUFFERED=1

RUN apt-get update && apt-get install -y curl && rm -rf /var/lib/apt/lists/*

# Optional Claude Code CLI for the claude_cli model provider.
ARG INSTALL_CLAUDE_CLI=false
ARG CLAUDE_CLI_VERSION=stable
ENV DISABLE_AUTOUPDATER=1
RUN if [ "$INSTALL_CLAUDE_CLI" = "true" ]; then \
        curl -fsSL https://claude.ai/install.sh | bash -s "$CLAUDE_CLI_VERSION" \
        && cp -L /root/.local/bin/claude /usr/local/bin/claude \
        && rm -rf /root/.local/bin/claude /root/.local/share/claude \
        && claude --version; \
    fi

WORKDIR /app

# Copy site-packages from builder
COPY --from=builder /usr/local/lib/python3.12/site-packages /usr/local/lib/python3.12/site-packages

# Copy application code
COPY . .

CMD ["sh", "-c", "python -m uvicorn main:app --host 0.0.0.0 --port ${PORT} --workers ${AI_WORKERS:-1}"]
