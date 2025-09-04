# Indexer Integration Tests

These tests require PostgreSQL and Redis to be running.

## Prerequisites

Start the required services:

```bash
# From the root directory
docker-compose up -d postgres redis
```

Or run them locally:

```bash
# PostgreSQL
docker run -d --name omni-postgres -p 5432:5432 -e POSTGRES_PASSWORD=postgres postgres:17

# Redis
docker run -d --name omni-redis -p 6379:6379 redis:7-alpine
```

## Running Tests

```bash
# Run all tests
cargo test

# Run tests sequentially (recommended for database tests)
cargo test -- --test-threads=1

# Run specific test
cargo test test_health_check

# Run with logging
RUST_LOG=info cargo test -- --nocapture
```

## Test Organization

- `api_integration_test.rs` - Tests REST API endpoints with real database
- `event_processor_test.rs` - Tests event processing via Redis Pub/Sub
- `common/mod.rs` - Shared test utilities and fixtures

Each test creates its own isolated test database and cleans up after completion.