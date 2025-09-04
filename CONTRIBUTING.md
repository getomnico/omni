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

## Code of Conduct

We are committed to providing a welcoming and inclusive environment. Please be respectful and considerate in all interactions.

## Getting Started

1. **Fork the repository** on GitHub
2. **Clone your fork** locally:
   ```bash
   git clone https://github.com/YOUR_USERNAME/omni.git
   cd omni
   ```
3. **Add upstream remote**:
   ```bash
   git remote add upstream https://github.com/omnihq/omni.git
   ```

## Development Setup

### Prerequisites

- **Rust** 1.75+ (install via [rustup](https://rustup.rs/))
- **Node.js** 20+ and npm
- **Python** 3.10+
- **Docker** and Docker Compose
- **PostgreSQL** 17+

### Initial Setup

1. **Install Rust dependencies**:
   ```bash
   cargo build --workspace
   ```

2. **Install frontend dependencies**:
   ```bash
   cd web
   npm install
   cd ..
   ```

3. **Install Python dependencies** (for AI service):
   ```bash
   cd services/ai
   python -m venv venv
   source venv/bin/activate  # On Windows: venv\Scripts\activate
   pip install -r requirements.txt
   cd ../..
   ```

4. **Start development environment**:
   ```bash
   docker compose -f docker-compose.yml -f docker-compose.dev.yml up
   ```

### Environment Configuration

Create a `.env` file in the root directory:

```env
DATABASE_URL=postgresql://omni:omni@localhost:5432/omni
REDIS_URL=redis://localhost:6379
RUST_LOG=debug
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
   git commit -m "feat: add new search filter capability"
   ```

### Commit Message Format

We follow the recommendations from the [Google Eng Practices Documentation](https://google.github.io/eng-practices/review/developer/cl-descriptions.html).

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

- Add unit tests for all new functionality
- Include integration tests for API endpoints
- Test error cases and edge conditions
- Aim for >80% code coverage

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

- **Discord**: Join our [community Discord](https://discord.gg/omni)
- **GitHub Issues**: For bug reports and feature requests
- **Discussions**: For questions and ideas

## Recognition

Contributors will be recognized in our:
- [Contributors list](https://github.com/omnihq/omni/graphs/contributors)
- Release notes
- Project documentation

Thank you for contributing to Omni! 🎉
