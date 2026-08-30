# Omni

Omni is an open-source, self-hosted AI agent for the workplace. It connects to the apps a team already uses (Google Drive, Gmail, Slack, Confluence, etc.), syncs their data into a unified BM25 and pgvector index, and provides an agent harness that can search, reason over, and act on that data using any major cloud or private (self-hosted) model provider.

## Core Philosophy

The core philosophy behind the project is simplicity: use Postgres (ParadeDB) as the primary application storage system and hybrid (text + vector) search index, minimizing operational complexity.

## Features

- Built-in connectors that sync workplace app data through source-appropriate polling or webhook mechanisms (Google Drive, Gmail, Slack, Confluence, Jira, and many more), with one container per connector
- Each connector can sync data across multiple related apps. E.g., the google connector syncs data from Drive, Gmail and Chat.
- An agent harness (omni-ai) that orchestrates LLM agents with tools for index search, file download, bash/python execution, and more, including auto-compaction, context management and memory
- A unified index (BM25 + pgvector) over all connected sources, with a search UI and LLM summarization of results
- Model flexibility: works with all major cloud LLM providers (OpenAI, Anthropic, Gemini, AWS Bedrock, etc.) as well as private/self-hosted models, plus pluggable embedding providers (Jina AI, Cohere, OpenAI, etc.)
- A web-based chat UI for interacting with the agent.

## Architecture

Omni's main application services run as separate containers, alongside one container per connector.

Why one container per connector? To allow developers full control over packaging their dependencies without affecting other connectors.

### Core Services & Code Organization

- All core services are under services/ and web/
   a. omni-searcher (services/searcher): handles all index search requests
   b. omni-indexer (services/indexer): handles all writes to the index 
   c. omni-ai (services/ai): orchestrates all LLM interactions, agents
   d. omni-web (web/): frontend SvelteKit app
   e. omni-connector-manager (services/connector-manager): orchestrates all connector containers
   f. omni-sandbox (services/sandbox): Sandbox container for bash and python execution, file manipulation
- One container per connector, built-in connectors are in connectors/
   a. omni-google-connector (drive & gmail)
   b. omni-slack-connector
   c. omni-atlassian-connector (confluence & jira), and so on
- Connector SDKs in Python, TypeScript and Rust under sdk/
- Database migrations: services/migrations.

### Deployment Configs

- Docker Compose (docker/)
- AWS and GCP Terraform (infra/aws, infra/gcp)

## Tech Stack

- omni-searcher, omni-indexer, omni-connector-manager, and omni-sandbox are written in Rust
- omni-ai is a Python service
- omni-web is a SvelteKit app
- Postgres (ParadeDB) and Redis

## How it works (at a glance)

1. Connectors pull data from workplace apps and emit events via the connector manager, which enqueues them in Postgres
2. The indexer consumes events and maintains the unified BM25 (ParadeDB) + pgvector document index
3. If an embedding provider is configured, omni-ai's batch processor writes embeddings for semantic search
4. omni-web calls omni-searcher for index queries; the agent harness (omni-ai) gives LLM agents tools for search, file download, and bash/python execution, then loops until it answers

## Core Principles

Some core development principles that should be kept in mind when developing Omni:

- One Writer Per Table: only one service should be responsible for writing to a table, but there can be multiple readers
- Connectors must never directly interact with *any* omni service. Everything goes through the connector manager, via the appropriate SDK method/API.

## Coding/Testing Principles

- Local development uses Docker Compose; see CONTRIBUTING.md for prerequisites. From the repository root:

  ```bash
  [ -f .env ] || cp .env.example .env
  docker compose -f docker/docker-compose.yml -f docker/docker-compose.dev.yml \
    --env-file .env up -d --build
  ```

  The web and AI services hot-reload. Their default URLs are http://localhost:3000 and http://localhost:3003; ports can be changed in `.env`. Fixed container names and host ports mean only one local stack can run at a time. To stop it, run the same command with `down` in place of `up -d --build`.

  For host-side frontend tooling in a fresh checkout, run `cd web && npm ci`.
- Wherever possible, add imports at the top of the source file, instead of using fully-qualified names in the middle of the file
- For local development database operations against this Compose stack, the container is `omni-postgres` and the database user is `omni_dev`; credentials are defined in `docker/docker-compose.dev.yml` and must not be assumed outside local development.
- Avoid adding comments that only talk about the *what*. That is already evident from reading the code. Only add comments where strongly needed to explain the *why* behind an implementation.
- In Svelte code, always use tailwind 'cursor-pointer' on buttons
- As much as possible, avoid using opaque types like list[Any], or data: any, etc. etc. Use proper concrete types wherever available. Or define your own if no type exists (either in our own libs, or from external deps).
- Do not mask missing required data with fallback values such as `model.get("content", "")`. Validate untrusted input and external API responses at their boundaries. If required data is absent or malformed, fail explicitly; represent genuinely optional data with appropriate optional types.
- Avoid using empty string "" to represent the "empty/missing" state. Use the lang appropriate types: Option in rust, Optional in python, null in TS/JS
- Define concrete types wherever possible, especially when handling the return type of an HTTP API call. As soon as we receive something over the wire, we should attempt to parse it into this concrete type and fail immediately if we are unable to (again, depending on severity). We should not use vague types like dict[str, Any], etc. unless creating a new type would be too much of an overkill.

### Testing:
- Prefer integration tests. Most services have an integ harness; use it.
- Use testcontainers with real Postgres/Redis (the existing infra already does this).
- Python connector suites use the harness in sdk/python/omni_connector/testing.

## Community and commercial editions

This repository is Apache 2.0 licensed and contains the community edition of Omni. The community edition should remain secure, useful, and operationally complete for self-hosting. Commercial offerings may add organizational governance, compliance workflows, enterprise IT integrations, and large-scale deployment capabilities.

For changes intended for contribution to this repository, the following capabilities currently require maintainer confirmation because they are expected to belong in the commercial edition:

- Enterprise SSO and identity federation (OIDC, SAML)
- SCIM lifecycle provisioning
- Advanced RBAC beyond basic admin/member roles
- Per-group source visibility
- Audit reporting and export
- Query history export
- Policy-driven data retention
- Centrally managed IP allow/block policies
- Large-scale HA/multi-node deployment features
- Custom branding and consent screens

This list may change as Omni evolves. Community code must include the functionality needed for secure self-hosting. Commercial code can add organization-wide administration, reporting, and scaling for the same feature. This guidance governs upstream contributions and does not restrict changes in downstream forks.

Before implementing a listed capability, or one that appears to fall into the same categories, for upstream contribution, stop and alert the developer that maintainer confirmation is required. Evaluate ambiguous features case by case: if its absence would make a normal self-hosted deployment materially less secure or operationally complete, default to the community edition. If its primary value is organizational control, compliance reporting, enterprise IT workflow integration, or large-scale operation, default to the commercial edition.

