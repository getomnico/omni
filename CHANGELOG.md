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
- Comprehensive integration test suite for indexer service
  - Real database and Redis connections without mocking for production-like testing
  - API integration tests covering all REST endpoints with isolated test databases
  - Event processor tests validating Redis pub/sub document lifecycle handling
  - End-to-end flow tests combining event processing and REST API operations
  - Test utilities with automatic database setup, migration, and cleanup
- Searcher service with PostgreSQL full-text search capabilities
  - POST /search endpoint with query processing, source filtering, and pagination support
  - GET /suggestions endpoint for autocomplete functionality based on document titles
  - Integration with existing database schema using generated tsvector columns and GIN indexes for fast text search
- Enhanced searcher service with semantic and hybrid search capabilities
  - Added configurable search modes: fulltext (PostgreSQL FTS), semantic (vector similarity), and hybrid (combined)
  - Implemented vector similarity search using pgvector and EmbeddingRepository for AI-powered semantic results
  - Hybrid search combines FTS and semantic results with weighted scoring (60% FTS, 40% semantic)
  - Clean API design with SearchMode enum replacing multiple boolean flags for better clarity
- Completed searcher service Phase 3 implementation
  - AI service integration for query embedding generation with HTTP client and fallback handling
  - Redis caching layer for search results with 5-minute TTL and query-based cache keys
  - Search result highlights extraction with context snippets and markdown formatting
  - Unit tests for search models and request validation
- Google Drive OAuth integration
  - OAuth flow implementation in SvelteKit backend with state management via Redis
  - Google connector service for automatic document syncing with token refresh
  - Secure token storage in PostgreSQL with encryption at rest
- Dedicated OAuth credentials table for multi-provider support
  - Separate oauth_credentials table supporting Google, Slack, Atlassian, GitHub, and Microsoft providers
  - Provider-specific metadata storage and automatic token expiration tracking for refresh workflows
- Enhanced Caddy reverse proxy configuration
  - Production-ready domain support with CLIO_DOMAIN environment variable
  - Health checks for all services with configurable intervals
  - Advanced security headers (CSP, HSTS, permissions policies)
  - Improved compression with zstd alongside gzip
  - Load balancing ready with round-robin policy for future scaling
- Modern authentication system with shadcn-svelte UI
  - SvelteKit route groups: (public) for auth routes, (app) for protected routes
  - Clean routing structure: /login and /signup instead of /auth/* paths
  - Professional UI components using shadcn-svelte with responsive design
  - Session-based authentication with secure cookies and Argon2 password hashing
  - ULID-based user IDs and role-based access control (admin/user/viewer)
  - Rate limiting, admin approval workflow, and user status management
- vLLM integration for local LLM inference
  - Added vLLM container with GPU support using official vllm/vllm-openai image
  - Configured with Microsoft Phi-3-mini-4k-instruct as default model with customizable options
- Organization-level admin OAuth flow for Google Workspace
  - Admin integrations management page at /admin/integrations with organization-wide data source control
  - Modified OAuth flow to support org-level connections instead of per-user, with admin-only access controls
  - User settings page at /settings/integrations for read-only view of connected data sources
- Switched from using Redis PubSub to Postgres table as message queue for indexing task processing
- Complete search UI implementation with Svelte 5
  - Interactive search interface on home page with real-time query handling
  - Dedicated search results page (/search) with document previews, highlights, and metadata display
  - Search API endpoints (/api/search, /api/search/suggestions) for SvelteKit integration with searcher service
  - TypeScript type definitions for search functionality with proper error handling and loading states
- Typo-tolerant search implementation
  - Database migration to enable fuzzystrmatch extension and create unique_lexemes materialized view
  - Levenshtein distance-based word correction for misspelled queries
  - Configurable typo tolerance with max distance and minimum word length settings
  - SearchResponse enhanced with corrected_query and corrections fields for user feedback
  - Background task in indexer to periodically refresh lexeme dictionary
  - Integration with existing fulltext and hybrid search modes
- Search facets for filtering and result counts
  - Added facet support with counts for source_type, content_type, last_updated time buckets, and owner
  - GIN indexes on documents.metadata and permissions fields for efficient facet queries
- Streaming AI-powered answers with RAG (Retrieval-Augmented Generation)
  - Real-time streaming from vLLM through clio-ai, clio-searcher, and clio-web to browser
  - Hybrid search (FTS + vector) for optimal RAG context extraction with top 5 documents
  - Citation system with [Source: Document Title] format for answer attribution
  - Frontend component with live streaming display and clickable source references
- Robust test infrastructure with testcontainers
  - Automatic dependency management using Docker containers for PostgreSQL and Redis
  - Mock AI service with HTTP endpoints for embeddings, RAG, and generation
  - TestEnvironment struct providing isolated test databases and cleanup for each test suite
  - GitHub Actions CI/CD workflow with parallel test execution and dependency caching
- PDF text extraction support in Google Drive connector using pdfium-render library

