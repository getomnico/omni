#!/usr/bin/env bash
set -euo pipefail

echo "==> Setting up Omni development environment..."

# Copy .env if it doesn't exist
if [ ! -f .env ]; then
  cp .env.example .env
  echo "    Created .env from .env.example"
fi

# Install frontend dependencies
echo "==> Installing web frontend dependencies..."
(cd web && npm install)

# Install AI service dependencies
echo "==> Installing AI service Python dependencies..."
(cd services/ai && uv sync)

echo "==> Dev environment ready!"
