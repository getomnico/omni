# Clio

<div align="center">

**The Open-Source Enterprise Search Platform**

*A self-hosted alternative to Glean with AI-powered semantic search*

[![License: Apache 2.0](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](https://opensource.org/licenses/Apache-2.0)
[![Rust](https://img.shields.io/badge/rust-%23000000.svg?style=flat&logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![Python](https://img.shields.io/badge/python-3670A0?style=flat&logo=python&logoColor=ffdd54)](https://www.python.org/)
[![Svelte](https://img.shields.io/badge/svelte-%23f1413d.svg?style=flat&logo=svelte&logoColor=white)](https://svelte.dev/)
[![PostgreSQL](https://img.shields.io/badge/postgres-%23316192.svg?style=flat&logo=postgresql&logoColor=white)](https://postgresql.org/)

[Features](#features) • [Architecture](#architecture) • [Quick Start](#quick-start) • [Contributing](#contributing)

</div>

---

## What is Clio?

Clio is a powerful, self-hosted enterprise search platform that unifies your organization's knowledge across multiple data sources. Think Elasticsearch meets ChatGPT, but completely under your control.

- **Privacy-first**: Your data never leaves your infrastructure
- **Lightning-fast**: Sub-second search across millions of documents
- **AI-powered**: Semantic search and document summarization using local LLMs
- **Easy setup**: Up and running in 30 minutes with Docker Compose

## Features

### Unified Search
- Search across Google Workspace, Slack, Confluence, GitHub, and more
- Combined full-text and semantic search with PostgreSQL + pgvector
- Real-time indexing with sub-second response times

### AI-Driven Intelligence
- Document summarization using local LLM models (no external APIs)
- Semantic search with state-of-the-art embeddings
- RAG (Retrieval-Augmented Generation) for context-aware answers

### Enterprise-Ready Security
- Role-based access control (RBAC) with source-level permissions
- OAuth integration for seamless authentication
- Complete audit logging for compliance

### Scalable Architecture
- Event-driven microservices for loose coupling
- Handles 5M+ documents efficiently with PostgreSQL
- Optional Elasticsearch upgrade path for larger deployments

## Architecture

Clio uses a modern, event-driven microservices architecture built for scalability and maintainability:

```
┌─────────────┐    ┌──────────────┐    ┌─────────────┐
│  SvelteKit  │───▶│    Search    │───▶│ PostgreSQL  │
│  Frontend   │    │   Service    │    │ + pgvector  │
└─────────────┘    └──────────────┘    └─────────────┘
                            │                   ▲
                            ▼                   │
┌─────────────┐    ┌──────────────┐    ┌───────┴─────┐
│ Connectors  │───▶│ Redis Pub/Sub│───▶│   Indexer   │
│ (Rust μSvc) │    │              │    │   Service   │
└─────────────┘    └──────────────┘    └─────────────┘
                                                │
                                                ▼
                                       ┌─────────────┐
                                       │ AI Service  │◀──┐
                                       │  (FastAPI)  │   │
                                       └─────────────┘   │
                                                         │
                                       ┌─────────────────┘
                                       │ vLLM Server
                                       │ (Local LLM)
```

### Core Components

- **Search Service** (Rust): Query processing, result ranking, caching
- **Indexer Service** (Rust): Document processing, database writes
- **AI Service** (Python): Embedding generation, RAG orchestration
- **Connectors** (Rust): Independent microservices for each data source
- **SvelteKit Frontend**: Modern web interface with TypeScript

## Quick Start

*Detailed deployment instructions coming soon! We're finalizing the k8s setup*

## Supported Integrations

### Phase 1 (Available)
- ✅ **Google Workspace** - Drive, Docs, Gmail
- ✅ **Slack** - Messages, files, channels
- ✅ **Confluence** - Pages, attachments, spaces

### Phase 2 (Coming Soon)
- **GitHub** - Repositories, issues, pull requests
- **Local Files** - File system indexing
- **Notion** - Pages and databases
- **Jira** - Issues and projects

## Tech Stack

| Component | Technology | Purpose |
|-----------|------------|---------|
| **Backend Services** | Rust + Axum | High-performance async services |
| **AI/ML** | Python + FastAPI | Embeddings and LLM orchestration |
| **Frontend** | SvelteKit + TypeScript | Modern reactive web interface |
| **Database** | PostgreSQL + pgvector | Full-text search + vector embeddings |
| **Cache/Queue** | Redis | Search cache and message queue |
| **Deployment** | Docker Compose | Single-command deployment |

## Performance

- **Search Speed**: Sub-second response times
- **Scale**: Efficiently handles 5M+ documents
- **Memory**: Optimized Rust services with minimal footprint
- **Storage**: Intelligent indexing with PostgreSQL FTS + vector search

## Contributing

We welcome contributions! Clio is built with modern tools and follows best practices:

- **Monorepo**: Cargo workspace for Rust services
- **Type Safety**: Full TypeScript + Rust type safety
- **Testing**: Comprehensive test suites with integration tests
- **CI/CD**: Automated testing and deployment

See [CONTRIBUTING.md](CONTRIBUTING.md) for development setup and guidelines.

## License

Clio is licensed under the Apache License 2.0. See [LICENSE](LICENSE) for details.

## Acknowledgments

Built with amazing open-source projects:
- [Axum](https://github.com/tokio-rs/axum) for Rust web services
- [SvelteKit](https://kit.svelte.dev/) for the frontend
- [pgvector](https://github.com/pgvector/pgvector) for vector search
- [Hugging Face](https://huggingface.co/) for embedding models

---

<div align="center">

**Ready to take control of your enterprise search?**

[⭐ Star this repo](https://github.com/yourusername/clio) • [Documentation](docs/) • [Discussions](https://github.com/yourusername/clio/discussions)

</div>
