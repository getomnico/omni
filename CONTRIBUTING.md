# Contributing to Omni

Thank you for your interest in contributing to Omni! This document provides guidelines and instructions for contributing to the project.

## Table of Contents

- [Code of Conduct](#code-of-conduct)
- [Getting Started](#getting-started)
- [Development Setup](#development-setup)
- [Project Structure](#project-structure)
- [Making Changes](#making-changes)
- [Testing](#testing)
- [Submitting Changes](#submitting-changes)
- [Style Guidelines](#style-guidelines)
- [Community](#community)

## Getting Started

1. **Fork the repository** on GitHub
2. **Clone your fork** locally:
   ```bash
   git clone https://github.com/YOUR_USERNAME/omni.git
   cd omni
   ```
3. **Add upstream remote**:
   ```bash
   git remote add upstream https://github.com/getomnico/omni.git
   ```

## Development Setup

### Prerequisites

- **Docker** and Docker Compose (primary requirement - all services run in containers)
- **Rust** 1.75+ (install via [rustup](https://rustup.rs/)) - only needed for local development outside containers
- **Node.js** 18+ - only needed for local frontend development
- **Python** 3.12+ - only needed for local AI service development

### Initial Setup

1. **Configure environment**:
   ```bash
   cp .env.example .env
   # Edit .env to add your API keys (LLM_API_KEY, EMBEDDING_API_KEY, etc.)
   ```

2. **Start development environment**:
   ```bash
   docker compose -f docker/docker-compose.yml -f docker/docker-compose.dev.yml --env-file .env up -d --build
   ```

3. **Access the application**:
   - Web UI: http://localhost:3000

### Development Workflow

- **Hot-reload services**: `omni-web` (SvelteKit) and `omni-ai` (Python/FastAPI) automatically reload when you edit their source files
- **Rust services**: Need to be rebuilt after changes:
  ```bash
  docker compose -f docker/docker-compose.yml -f docker/docker-compose.dev.yml --env-file .env up -d --build searcher
  ```

### Local Development (Optional)

If you prefer developing outside containers:

**Rust services**:
```bash
cargo build --workspace
```

**Frontend**:
```bash
cd web
npm install
```

**AI service**:
```bash
cd services/ai
uv sync
```

## Project Structure

```
omni/
├── services/          # Core microservices
│   ├── searcher/      # Search query processing (Rust)
│   ├── indexer/       # Document indexing (Rust)
│   └── ai/            # AI/ML service (Python)
├── connectors/        # Data source connectors (Rust)
│   ├── google/
│   ├── slack/
│   └── atlassian/
├── web/               # SvelteKit frontend
├── shared/            # Shared Rust libraries
└── scripts/           # Build and deployment scripts
```

## Making Changes

### Workflow

1. **Create a feature branch**:
   ```bash
   git checkout -b feature/your-feature-name
   ```

2. **Make your changes** following our guidelines

3. **Test your changes** thoroughly

4. **Commit with meaningful messages**:
   ```bash
   git commit -m "Add new search filter capability"
   ```

## Testing

### Running Tests

**Rust tests**:
```bash
# Run all tests
cargo test --workspace

# Run specific service tests
cargo test -p indexer

# Run with logs
RUST_LOG=debug cargo test --workspace
```

**Frontend tests**:
```bash
cd web
npm test
npm run test:e2e  # End-to-end tests
```

**Python tests**:
```bash
cd services/ai
pytest
```

### Writing Tests

- Prefer integration tests
- Avoid unnecessary unit tests for the sake of coverage

## Submitting Changes

### Pull Request Process

1. **Update your fork**:
   ```bash
   git fetch upstream
   git rebase upstream/main
   ```

2. **Push to your fork**:
   ```bash
   git push origin feature/your-feature-name
   ```

3. **Create a Pull Request** on GitHub

4. **PR Requirements**:
   - Clear description of changes
   - Reference any related issues
   - All tests passing
   - Code follows style guidelines
   - Documentation updated if needed

### Review Process

- PRs require at least one maintainer approval
- Address all review feedback
- Keep PRs focused and reasonably sized
- Be patient - reviews may take a few days

## Style Guidelines

### Rust Code

- Follow standard Rust conventions
- Use `cargo fmt` before committing
- Run `cargo clippy` and address warnings
- Add doc comments for public APIs

Example:
```rust
/// Processes a search query and returns ranked results
///
/// # Arguments
/// * `query` - The search query string
/// * `filters` - Optional search filters
///
/// # Returns
/// A vector of search results ordered by relevance
pub async fn search(
    query: &str,
    filters: Option<SearchFilters>,
) -> Result<Vec<SearchResult>, SearchError> {
    // Implementation
}
```

### TypeScript/Svelte

- Use TypeScript strict mode
- Follow the existing code style
- Prefer composition over inheritance
- Use descriptive variable names

### Python Code

- Follow PEP 8
- Use type hints
- Add docstrings for functions and classes
- Keep functions focused and small

## Getting Help

- **Discord**: Join our [community Discord](https://discord.gg/Ja4xwxmM)
- **GitHub Issues**: For bug reports and feature requests
- **Discussions**: For questions and ideas

## Recognition

Contributors will be recognized in our:
- Contributors list
- Release notes
- Project documentation

Thank you for contributing to Omni! 🎉
