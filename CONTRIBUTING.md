# Contributing to Omni

## Getting Started

1. Fork the repository on GitHub
2. Clone your fork locally:
   ```bash
   git clone https://github.com/<your_gh_username>/omni.git
   cd omni
   ```
3. Add upstream remote:
   ```bash
   git remote add upstream https://github.com/getomnico/omni.git
   ```

## Development Setup

### Prerequisites

- **Docker** and Docker Compose (primary requirement — all services run in containers)
- **Rust** 1.75+ (install via [rustup](https://rustup.rs/)) — only needed for development outside containers
- **Node.js** 22+ — only needed for frontend development outside containers
- **Python** 3.12+ and [uv](https://docs.astral.sh/uv/) — only needed for AI service development outside containers

### Initial Setup

1. Configure environment:
   ```bash
   cp .env.example .env
   ```

2. Start the development environment:
   ```bash
   docker compose -f docker/docker-compose.yml -f docker/docker-compose.dev.yml --env-file .env up -d --build
   ```

3. Access the web UI at http://localhost:3000

### Development Workflow

- `omni-web` (SvelteKit) and `omni-ai` (Python/FastAPI) hot-reload when you edit source files
- Rust services need to be rebuilt after changes:
  ```bash
  docker compose -f docker/docker-compose.yml -f docker/docker-compose.dev.yml --env-file .env up -d --build searcher
  ```

### Local Development (Optional)

If you prefer developing outside containers:

```bash
# Rust services
cargo build --workspace

# Frontend
cd web && npm install

# AI service
cd services/ai && uv sync
```

## Project Structure

```
omni/
├── services/
│   ├── searcher/               # Search engine (Rust)
│   ├── indexer/                # Document indexing (Rust)
│   ├── ai/                    # LLM orchestration, agent (Python)
│   ├── connector-manager/     # Connector orchestration (Rust)
│   ├── sandbox/               # Code execution sandbox (Rust)
│   └── migrations/            # SQL migrations
├── connectors/                # One container per data source
│   ├── google/                #   Google Drive & Gmail (Rust)
│   ├── slack/                 #   Slack (Rust)
│   ├── atlassian/             #   Confluence & Jira (Rust)
│   └── ...
├── web/                       # SvelteKit frontend
├── sdk/                       # Connector SDKs (Python, TypeScript)
├── shared/                    # Shared Rust libraries
└── docker/                    # Compose files
```

## Testing

### Running Tests

**Rust:**
```bash
cargo test --workspace

# Specific service
cargo test -p indexer

# With logs
RUST_LOG=debug cargo test --workspace
```

**Python (AI service):**
```bash
cd services/ai
uv run pytest
```

### Writing Tests

- Prefer integration tests over unit tests. We use testcontainers to bring up real Postgres (ParadeDB) and Redis instances — the existing test harnesses in most services already do this.
- For Python connector tests, there's a testing harness at `sdk/python/omni_connector/testing`.
- Avoid unit tests just for coverage. If the behavior is better tested against a real database instance, do that instead.

## Submitting Changes

1. Create a feature branch:
   ```bash
   git checkout -b feature/your-feature-name
   ```
2. Make your changes and test against the local dev deployment
3. Update your fork and push:
   ```bash
   git fetch upstream
   git rebase upstream/main
   git push origin feature/your-feature-name
   ```
4. Create a Pull Request on GitHub

## Getting Help

- **Discord**: [Community Discord](https://discord.gg/aNr2J3xD)
- **GitHub Issues**: Bug reports and feature requests
- **Discussions**: Questions and ideas
