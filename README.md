<div align="center">

[![Discord](https://img.shields.io/badge/discord-join-5865F2?logo=discord&logoColor=white)](https://discord.gg/aNr2J3xD)

<img width="64" height="64" alt="omni-logo-256" src="https://github.com/user-attachments/assets/981ef763-41d5-4ae1-9cf8-a97d2e601c81#gh-light-mode-only" />
<img width="64" height="64" alt="omni-logo-dark-256" src="https://github.com/user-attachments/assets/5d3fb1c2-ced0-433a-86a1-8b4e6005fb4f#gh-dark-mode-only" />

**Omni is an AI Assistant and Search platform for the Workplace.**

Connects to your workplace apps, helps employees find information and get work done.

[Features](#features) • [Architecture](#architecture) • [Docs](https://docs.getomni.co) • [Deploy](#deployment) • [Contributing](#contributing)

</div>

![Omni Demo](.github/assets/omni_2.avif)

---

## Features

- **Unified Search**: Connect Google Drive/Gmail, Slack, Confluence, Jira, and more. Full-text (BM25) and semantic (pgvector) search across all of them.
- **AI Agent**: Chat interface with tool use: searches your connected apps, reads documents, and executes Python/bash in a sandboxed container to analyze data.
- **Self-hosted**: Runs entirely on your infrastructure. No data leaves your network.
- **Permission Inheritance**: Respects source system permissions. Users only see data they're already authorized to access.
- **Bring Your Own LLM**: Anthropic, OpenAI, Gemini, or open-weight models via vLLM.
- **Simple Deployment**: Docker Compose for single-server setups, Terraform for production AWS/GCP deployments.

## Architecture

Omni uses **Postgres ([ParadeDB](https://paradedb.com))** for everything: BM25 full-text search, pgvector semantic search, and all application data. No Elasticsearch, no dedicated vector database. One database to tune, backup, and monitor.

Core services are written in **Rust** (searcher, indexer, connector-manager), **Python** (AI/LLM orchestration), and **SvelteKit** (web frontend). Each data source connector runs as its own lightweight container, allowing connectors to use different languages and dependencies without affecting each other.

The AI agent can execute code in a sandboxed container that runs on an isolated Docker network (no access to internal services or the internet), with Landlock filesystem restrictions, resource limits, and a read-only root filesystem.

See the full [architecture documentation](https://docs.getomni.co/architecture) for more details.

## Deployment

Omni can be deployed entirely on your own infra. See our deployment guides:

- [Docker Compose](https://docs.getomni.co/deployment/docker-compose)
- [Terraform (AWS/GCP)](https://docs.getomni.co/deployment/aws-terraform)

## Supported Integrations

- **Google Workspace**: Drive, Gmail
- **Slack**: Messages, files, public channels
- **Confluence**: Pages, attachments, spaces
- **Jira**: Issues and projects
- **Web**: Public websites, documentation and help pages
- **Fireflies**: Meeting transcripts
- **HubSpot**: Contacts, companies, deals, tickets
- **Local Files**: File system indexing

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for development setup and guidelines.

## License

Apache License 2.0. See [LICENSE](LICENSE) for details.

---

<div align="center">

[Documentation](https://docs.getomni.co) • [Discord](https://discord.gg/aNr2J3xD) • [Discussions](https://github.com/getomnico/omni/discussions)

</div>
