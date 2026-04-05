FROM mcr.microsoft.com/devcontainers/base:ubuntu-24.04

ARG RUST_VERSION=1.86.0
ARG NODE_VERSION=22
ARG PYTHON_VERSION=3.12
ARG UV_VERSION=0.10.9

# Avoid prompts during package installation
ENV DEBIAN_FRONTEND=noninteractive

# Install system dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential \
    pkg-config \
    libssl-dev \
    libpq-dev \
    curl \
    git \
    ca-certificates \
    gnupg \
    unzip \
    && rm -rf /var/lib/apt/lists/*

# Install Rust as the vscode user
USER vscode
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y \
    --default-toolchain ${RUST_VERSION} \
    && . "$HOME/.cargo/env" \
    && rustup component add rust-src rust-analyzer clippy rustfmt
ENV PATH="/home/vscode/.cargo/bin:${PATH}"

# Install Node.js via nvm (already set up in devcontainer base image)
USER root
RUN curl -fsSL https://deb.nodesource.com/setup_${NODE_VERSION}.x | bash - \
    && apt-get install -y nodejs \
    && rm -rf /var/lib/apt/lists/*

# Install Python and uv
RUN apt-get update && apt-get install -y --no-install-recommends \
    python${PYTHON_VERSION} \
    python${PYTHON_VERSION}-dev \
    python${PYTHON_VERSION}-venv \
    python3-pip \
    && rm -rf /var/lib/apt/lists/* \
    && ln -sf /usr/bin/python${PYTHON_VERSION} /usr/bin/python3 \
    && ln -sf /usr/bin/python3 /usr/bin/python

ADD --chmod=755 https://astral.sh/uv/${UV_VERSION}/install.sh /tmp/uv-installer.sh
RUN su vscode -c "/tmp/uv-installer.sh" && rm /tmp/uv-installer.sh
ENV PATH="/home/vscode/.local/bin:${PATH}"

# Docker CLI is installed via the docker-outside-of-docker devcontainer feature

USER vscode
