# Architecture Overview

This document provides a detailed technical overview of Omni's architecture, designed to help IT teams understand the system design, plan deployments, and make informed decisions about scaling and integration.

## High-Level Architecture

Omni follows a microservices architecture with clear separation of concerns:

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                              External Layer                                 │
│                                                                             │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐        │
│  │   Google    │  │    Slack    │  │ Confluence  │  │   GitHub    │        │
│  │ Workspace   │  │             │  │             │  │             │        │
│  └─────────────┘  └─────────────┘  └─────────────┘  └─────────────┘        │
│         │                │                │                │               │
│         │                │                │                │               │
│         ▼                ▼                ▼                ▼               │
└─────────────────────────────────────────────────────────────────────────────┘
          │                │                │                │
          │                │                │                │
          ▼                ▼                ▼                ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│                            Connector Layer                                 │
│                                                                             │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐        │
│  │   Google    │  │    Slack    │  │ Confluence  │  │   GitHub    │        │
│  │ Connector   │  │ Connector   │  │ Connector   │  │ Connector   │        │
│  │   (Rust)    │  │   (Rust)    │  │   (Rust)    │  │   (Rust)    │        │
│  └─────────────┘  └─────────────┘  └─────────────┘  └─────────────┘        │
│         │                │                │                │               │
│         │                │                │                │               │
│         ▼                ▼                ▼                ▼               │
└─────────────────────────────────────────────────────────────────────────────┘
                                    │
                                    ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│                             Message Queue                                  │
│                                                                             │
│                     ┌─────────────────────────────────┐                    │
│                     │         PostgreSQL              │                    │
│                     │      Events Queue Table         │                    │
│                     └─────────────────────────────────┘                    │
│                                    │                                       │
│                                    ▼                                       │
└─────────────────────────────────────────────────────────────────────────────┘
                                    │
                                    ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│                            Processing Layer                                │
│                                                                             │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐        │
│  │    Web      │  │  Searcher   │  │   Indexer   │  │     AI      │        │
│  │ (SvelteKit) │  │   (Rust)    │  │   (Rust)    │  │  (Python)   │        │
│  │             │  │             │  │             │  │             │        │
│  │ • Frontend  │  │ • Query     │  │ • Document  │  │ • Embeddings│        │
│  │ • Auth      │  │   Processing│  │   Processing│  │ • RAG       │        │
│  │ • API       │  │ • Ranking   │  │ • DB Writes │  │ • LLM       │        │
│  └─────────────┘  └─────────────┘  └─────────────┘  └─────────────┘        │
│         │                │                │                │               │
│         │                │                │                │               │
│         ▼                ▼                ▼                ▼               │
└─────────────────────────────────────────────────────────────────────────────┘
                                    │
                                    ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│                             Data Layer                                     │
│                                                                             │
│            ┌─────────────┐              ┌─────────────┐                     │
│            │ PostgreSQL  │              │    Redis    │                     │
│            │             │              │             │                     │
│            │ • Documents │              │ • Cache     │                     │
│            │ • Users     │              │ • Sessions  │                     │
│            │ • Embeddings│              │ • Temp Data │                     │
│            │ • Audit Log │              │             │                     │
│            └─────────────┘              └─────────────┘                     │
└─────────────────────────────────────────────────────────────────────────────┘
```

## Core Components

### 1. Web Service (omni-web)

**Technology**: SvelteKit with TypeScript
**Purpose**: Frontend interface and API gateway

**Responsibilities**:
- User authentication and session management
- Web interface for search and administration
- API endpoints for mobile/external integrations
- Rate limiting and request routing
- OAuth flow management for data sources

**Key Features**:
- Server-side rendering for performance
- Real-time search with WebSocket support
- Role-based access control (RBAC)
- Responsive design for mobile devices
- Integration with external identity providers

**Resource Usage**:
- CPU: 1-2 cores
- Memory: 1-2 GB
- Storage: Minimal (app code only)

### 2. Searcher Service (omni-searcher)

**Technology**: Rust with Axum framework
**Purpose**: Search query processing and result ranking

**Responsibilities**:
- Parse and optimize search queries
- Execute full-text search against PostgreSQL
- Perform semantic search using pgvector
- Rank and score search results
- Cache frequently accessed results
- Generate search suggestions

**Key Features**:
- Sub-second search response times
- Typo tolerance and fuzzy matching
- Faceted search capabilities
- Personalized result ranking
- Search analytics and logging

**Resource Usage**:
- CPU: 2-4 cores
- Memory: 2-4 GB
- Storage: Minimal (cache only)

### 3. Indexer Service (omni-indexer)

**Technology**: Rust with Tokio async runtime
**Purpose**: Document processing and database writes

**Responsibilities**:
- Process documents from event queue
- Extract text content and metadata
- Generate document embeddings via AI service
- Store documents and embeddings in database
- Handle incremental updates and deletions
- Manage database schema migrations

**Key Features**:
- Concurrent document processing
- Retry logic with exponential backoff
- Duplicate detection and handling
- Batch processing for efficiency
- Health monitoring and metrics

**Resource Usage**:
- CPU: 1-2 cores
- Memory: 1-2 GB
- Storage: Temporary processing space

### 4. AI Service (omni-ai)

**Technology**: Python with FastAPI
**Purpose**: Machine learning and AI operations

**Responsibilities**:
- Generate document embeddings using transformer models
- Provide RAG (Retrieval-Augmented Generation) for AI answers
- Communicate with local LLM service (vLLM)
- Chunk documents for optimal processing
- Handle ML model lifecycle management

**Key Features**:
- Local model inference (no external APIs)
- Efficient embedding generation
- Contextual AI answer generation
- Model versioning and updates
- GPU acceleration support

**Resource Usage**:
- CPU: 2-4 cores (4-8 with GPU)
- Memory: 8-16 GB
- Storage: Model storage (2-10 GB)
- GPU: Optional but recommended

### 5. Data Sources (Connectors)

**Technology**: Rust microservices
**Purpose**: Integration with external data sources

**Responsibilities**:
- Authenticate with external APIs using OAuth
- Fetch documents and metadata
- Handle API rate limiting
- Transform data to common format
- Publish events to message queue
- Manage incremental synchronization

**Supported Connectors**:
- **Google Workspace**: Drive, Docs, Gmail, Calendar
- **Slack**: Messages, files, channels
- **Confluence**: Pages, attachments, spaces
- **GitHub**: Repositories, issues, pull requests

**Key Features**:
- Independent scaling per data source
- Fault tolerance and retry logic
- Permission mapping and inheritance
- Webhook support for real-time updates
- OAuth token refresh management

**Resource Usage** (per connector):
- CPU: 0.5-1 core
- Memory: 512 MB - 1 GB
- Storage: OAuth tokens and sync state

## Data Flow

### Document Indexing Pipeline

```
┌─────────────┐    ┌─────────────┐    ┌─────────────┐    ┌─────────────┐
│   External  │    │  Connector  │    │   Queue     │    │   Indexer   │
│     API     │    │             │    │             │    │             │
│             │    │ 1. Fetch    │    │ 2. Event    │    │ 3. Process  │
│ • Google    │───►│    Document │───►│    Queue    │───►│    Document │
│ • Slack     │    │             │    │             │    │             │
│ • etc.      │    │             │    │             │    │             │
└─────────────┘    └─────────────┘    └─────────────┘    └─────────────┘
                                                                 │
                                                                 ▼
┌─────────────┐    ┌─────────────┐    ┌─────────────┐    ┌─────────────┐
│ PostgreSQL  │    │      AI     │    │   Indexer   │    │   Indexer   │
│             │    │   Service   │    │             │    │             │
│ 6. Store    │◄───│ 5. Generate │◄───│ 4. Extract  │    │             │
│    Document │    │  Embeddings │    │    Content  │    │             │
│             │    │             │    │             │    │             │
└─────────────┘    └─────────────┘    └─────────────┘    └─────────────┘
```

### Search Query Processing

```
┌─────────────┐    ┌─────────────┐    ┌─────────────┐    ┌─────────────┐
│    User     │    │     Web     │    │  Searcher   │    │ PostgreSQL  │
│             │    │             │    │             │    │             │
│ 1. Search   │───►│ 2. Parse    │───►│ 3. Execute  │───►│ 4. Query    │
│    Query    │    │    Request  │    │    Search   │    │   Database  │
│             │    │             │    │             │    │             │
└─────────────┘    └─────────────┘    └─────────────┘    └─────────────┘
       ▲                                       │                   │
       │                                       ▼                   ▼
┌─────────────┐    ┌─────────────┐    ┌─────────────┐    ┌─────────────┐
│    User     │    │     Web     │    │  Searcher   │    │     AI      │
│             │    │             │    │             │    │   Service   │
│ 8. Results  │◄───│ 7. Format   │◄───│ 6. Rank     │◄───│ 5. Generate │
│             │    │   Response  │    │   Results   │    │  AI Answer  │
│             │    │             │    │             │    │             │
└─────────────┘    └─────────────┘    └─────────────┘    └─────────────┘
```

## Database Schema

### Core Tables

```sql
-- Users and authentication
CREATE TABLE users (
    id SERIAL PRIMARY KEY,
    email VARCHAR(255) UNIQUE NOT NULL,
    name VARCHAR(255) NOT NULL,
    role VARCHAR(50) NOT NULL DEFAULT 'user',
    created_at TIMESTAMP DEFAULT NOW(),
    last_login TIMESTAMP
);

-- Data sources configuration
CREATE TABLE sources (
    id SERIAL PRIMARY KEY,
    name VARCHAR(255) NOT NULL,
    type VARCHAR(50) NOT NULL, -- 'google', 'slack', etc.
    config JSONB NOT NULL,
    enabled BOOLEAN DEFAULT true,
    created_at TIMESTAMP DEFAULT NOW(),
    updated_at TIMESTAMP DEFAULT NOW()
);

-- Document storage
CREATE TABLE documents (
    id SERIAL PRIMARY KEY,
    source_id INTEGER REFERENCES sources(id),
    external_id VARCHAR(255) NOT NULL,
    title TEXT NOT NULL,
    content TEXT NOT NULL,
    content_tsvector TSVECTOR, -- For full-text search
    metadata JSONB,
    permissions JSONB,
    created_at TIMESTAMP DEFAULT NOW(),
    updated_at TIMESTAMP DEFAULT NOW(),
    indexed_at TIMESTAMP DEFAULT NOW()
);

-- Vector embeddings
CREATE TABLE embeddings (
    id SERIAL PRIMARY KEY,
    document_id INTEGER REFERENCES documents(id) ON DELETE CASCADE,
    embedding vector(1024), -- pgvector type
    chunk_index INTEGER DEFAULT 0,
    chunk_text TEXT,
    created_at TIMESTAMP DEFAULT NOW()
);

-- Event queue for reliable processing
CREATE TABLE connector_events_queue (
    id SERIAL PRIMARY KEY,
    event_type VARCHAR(50) NOT NULL,
    source_id INTEGER REFERENCES sources(id),
    payload JSONB NOT NULL,
    status VARCHAR(20) DEFAULT 'pending',
    attempts INTEGER DEFAULT 0,
    max_attempts INTEGER DEFAULT 3,
    created_at TIMESTAMP DEFAULT NOW(),
    processed_at TIMESTAMP,
    error_message TEXT
);

-- OAuth credentials
CREATE TABLE oauth_credentials (
    id SERIAL PRIMARY KEY,
    source_id INTEGER REFERENCES sources(id),
    access_token TEXT NOT NULL,
    refresh_token TEXT,
    expires_at TIMESTAMP,
    created_at TIMESTAMP DEFAULT NOW(),
    updated_at TIMESTAMP DEFAULT NOW()
);
```

### Indexes for Performance

```sql
-- Full-text search indexes
CREATE INDEX idx_documents_content_tsvector 
ON documents USING gin(content_tsvector);

-- Vector similarity search
CREATE INDEX idx_embeddings_vector 
ON embeddings USING ivfflat (embedding vector_cosine_ops);

-- Query optimization indexes
CREATE INDEX idx_documents_source_created 
ON documents(source_id, created_at);

CREATE INDEX idx_documents_external_id 
ON documents(source_id, external_id);

-- Event queue processing
CREATE INDEX idx_queue_status_created 
ON connector_events_queue(status, created_at);
```

## Message Queue System

Omni uses PostgreSQL as a reliable message queue instead of external systems like Redis Pub/Sub:

### Benefits
- **ACID Compliance**: Guaranteed message delivery
- **Simplicity**: No additional infrastructure
- **Reliability**: Built-in retry and dead letter queue
- **Monitoring**: Standard SQL queries for queue status

### Event Types
```rust
pub enum ConnectorEvent {
    DocumentCreated {
        source: String,
        document_id: String,
        content: String,
        metadata: Value,
        permissions: Vec<String>,
    },
    DocumentUpdated {
        source: String,
        document_id: String,
        content: String,
        metadata: Value,
    },
    DocumentDeleted {
        source: String,
        document_id: String,
    },
}
```

## Security Architecture

### Authentication Flow
```
┌─────────────┐    ┌─────────────┐    ┌─────────────┐    ┌─────────────┐
│    User     │    │    Omni     │    │  Identity   │    │ Data Source │
│             │    │     Web     │    │  Provider   │    │             │
│ 1. Login    │───►│ 2. Redirect │───►│ 3. OAuth    │    │             │
│   Request   │    │  to OAuth   │    │    Flow     │    │             │
└─────────────┘    └─────────────┘    └─────────────┘    └─────────────┘
       ▲                   │                   │                   │
       │                   ▼                   ▼                   ▼
┌─────────────┐    ┌─────────────┐    ┌─────────────┐    ┌─────────────┐
│    User     │    │    Omni     │    │  Identity   │    │ Data Source │
│             │    │     Web     │    │  Provider   │    │             │
│ 6. Access   │◄───│ 5. Session  │◄───│ 4. Return   │    │             │
│   Granted   │    │   Cookie    │    │   Token     │    │             │
└─────────────┘    └─────────────┘    └─────────────┘    └─────────────┘
```

### Permission Model
- **User Roles**: Admin, User, Read-only
- **Source Permissions**: Inherited from external systems
- **Document Access**: Checked at query time
- **API Access**: Rate-limited by user role

## Network Architecture

### Internal Communications
```
┌─────────────────────────────────────────────────────────────────────────────┐
│                         Docker Network (omni_default)                      │
│                                                                             │
│  ┌─────────────┐    ┌─────────────┐    ┌─────────────┐    ┌─────────────┐  │
│  │    Caddy    │◄──►│  omni-web   │◄──►│omni-searcher│◄──►│ PostgreSQL  │  │
│  │   :80/443   │    │    :3000    │    │    :8080    │    │   :5432     │  │
│  └─────────────┘    └─────────────┘    └─────────────┘    └─────────────┘  │
│                             │                                               │
│  ┌─────────────┐    ┌─────────────┐    ┌─────────────┐    ┌─────────────┐  │
│  │    Redis    │◄──►│omni-indexer │◄──►│   omni-ai   │◄──►│    vLLM     │  │
│  │   :6379     │    │    :8081    │    │    :8000    │    │   :8080     │  │
│  └─────────────┘    └─────────────┘    └─────────────┘    └─────────────┘  │
└─────────────────────────────────────────────────────────────────────────────┘
```

### External Communications
- **HTTPS (443)**: User access, OAuth callbacks
- **HTTP (80)**: Redirect to HTTPS
- **Outbound HTTPS**: API calls to data sources
- **DNS**: Service discovery and certificate validation

## Scaling Considerations

### Vertical Scaling (Single Server)
- **CPU**: Add cores for AI processing
- **Memory**: Increase for larger working sets
- **Storage**: Upgrade to faster NVMe for I/O
- **Network**: 10Gbps for high-throughput scenarios

### Horizontal Scaling (Multi-Server)
- **Load Balancer**: Multiple web service instances
- **Database**: Managed PostgreSQL with read replicas
- **Cache**: Redis cluster for distributed caching
- **AI Service**: Separate GPU servers for ML workloads

### Performance Optimization
- **Caching**: Multi-layer caching strategy
- **Indexing**: Optimized database indexes
- **Batching**: Batch processing for efficiency
- **Connection Pooling**: Database connection management

## Monitoring Points

### Health Checks
- Service container health
- Database connection health
- External API availability
- Queue processing status

### Key Metrics
- Search latency (p95, p99)
- Indexing throughput
- Database query performance
- Memory and CPU usage
- Error rates and types

### Alerting Thresholds
- Service downtime > 1 minute
- Search latency > 2 seconds
- Database connections > 80%
- Queue backlog > 10,000 items
- Disk usage > 85%

## Next Steps

For deployment planning:
1. **[System Requirements](../getting-started/system-requirements)** - Hardware sizing
2. **[Production Setup](../deployment/production-setup)** - Deployment configuration
3. **[Monitoring Guide](../operations/monitoring)** - Operational monitoring
4. **[Docker Deployment](../getting-started/docker-deployment)** - Quick deployment guide