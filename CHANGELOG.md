# Changelog

All notable changes to the Clio project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- Initial project scaffolding with Rust workspace configuration
- Database schema design with PostgreSQL migrations
  - V1: Users table
  - V2: Sources table for data source configurations
  - V3: Documents table for content storage
  - V4: Embeddings table with pgvector support
- Shared Rust crate with core data models
  - User, Source, Document, and Embedding structs with Serde support
- Database access layer with CRUD operations
  - Implemented repository pattern for all entities
  - PostgreSQL connection pooling with sqlx
  - Type-safe database operations for User, Source, Document, and Embedding entities
- Indexer service core infrastructure
  - HTTP server with health check endpoint and Redis connectivity
  - Database migration system for automatic schema setup
- Event-driven document processing system
  - Redis pub/sub subscriber for connector events (DocumentCreated, DocumentUpdated, DocumentDeleted)
  - Background event processor with automatic search vector generation and database updates
- Complete REST API for indexer service document management
  - POST /documents endpoint for manual document indexing with full metadata support
  - GET /documents/{id} endpoint for document retrieval with proper error handling
  - PUT /documents/{id} and DELETE /documents/{id} endpoints for document lifecycle management
  - POST /documents/bulk endpoint supporting batch operations (create/update/delete) for efficient processing