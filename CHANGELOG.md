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