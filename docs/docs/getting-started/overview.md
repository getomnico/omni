# Overview

Omni is an open-source, self-hosted enterprise search platform designed to unify your organization's knowledge across multiple data sources. This overview provides IT teams with the key information needed to evaluate and deploy Omni.

## What is Omni?

Omni aggregates and searches across your organization's data sources:

### Supported Data Sources
- **Google Workspace**: Drive, Docs, Sheets, Gmail, Calendar
- **Slack**: Messages, files, channels, DMs
- **Confluence**: Pages, attachments, spaces, comments
- **GitHub**: Repositories, issues, pull requests, wikis
- **Local Files**: File system indexing (planned)

### Key Capabilities
- **Unified Search**: Single interface for all your data sources
- **AI-Powered Answers**: Get contextual answers using local LLMs
- **Real-time Indexing**: Changes are searchable within minutes
- **Semantic Search**: Find relevant content even with different terminology
- **Access Control**: Respects source permissions and adds RBAC

## Why Choose Omni?

### For IT Teams

| Requirement | Omni Solution |
|-------------|---------------|
| **Data Privacy** | Fully self-hosted, no data leaves your infrastructure |
| **Compliance** | Built-in audit logging, configurable retention policies |
| **Integration** | OAuth with existing identity providers |
| **Scalability** | Handles 1M+ documents efficiently |
| **Maintenance** | Docker-based deployment, automated updates |
| **Cost** | Open source, no per-user licensing fees |

### vs. Cloud Solutions (Glean, etc.)

| Feature | Omni | Cloud Solutions |
|---------|------|-----------------|
| **Data Control** | ✅ Complete control | ❌ Data stored externally |
| **Privacy** | ✅ No data sharing | ❌ Subject to vendor policies |
| **Customization** | ✅ Full source code access | ❌ Limited customization |
| **Cost** | ✅ One-time deployment | ❌ Monthly per-user fees |
| **Compliance** | ✅ Your infrastructure | ❓ Vendor-dependent |

## Architecture Overview

Omni uses a microservices architecture optimized for reliability and performance:

```
┌─────────────────┐    ┌──────────────────┐    ┌─────────────────┐
│   Data Sources  │    │      Users       │    │   Admins        │
│                 │    │                  │    │                 │
│ • Google Drive  │    │ • Web Interface  │    │ • Management    │
│ • Slack         │    │ • Search API     │    │ • Monitoring    │
│ • Confluence    │    │ • Mobile Apps    │    │ • Configuration │
│ • GitHub        │    │                  │    │                 │
└─────────────────┘    └──────────────────┘    └─────────────────┘
         │                       │                       │
         │                       │                       │
         ▼                       ▼                       ▼
┌─────────────────────────────────────────────────────────────────┐
│                        Omni Platform                           │
│                                                                 │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐             │
│  │    Web      │  │  Searcher   │  │   Indexer   │             │
│  │ (Frontend)  │  │ (Search)    │  │ (Processing)│             │
│  └─────────────┘  └─────────────┘  └─────────────┘             │
│                                                                 │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐             │
│  │     AI      │  │ PostgreSQL  │  │    Redis    │             │
│  │ (ML/LLM)    │  │ (Database)  │  │  (Cache)    │             │
│  └─────────────┘  └─────────────┘  └─────────────┘             │
│                                                                 │
│  ┌─────────────────────────────────────────────────────────┐   │
│  │                  Connectors                             │   │
│  │  Google  │  Slack  │  Confluence  │  GitHub  │  ...     │   │
│  └─────────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────────┘
```

### Core Components

| Component | Purpose | Technology | Resource Usage |
|-----------|---------|------------|----------------|
| **omni-web** | Web interface, authentication, API gateway | SvelteKit/Node.js | 1-2 GB RAM |
| **omni-searcher** | Query processing, result ranking | Rust | 2-4 GB RAM |
| **omni-indexer** | Document processing, database writes | Rust | 1-2 GB RAM |
| **omni-ai** | Embeddings, AI answers, ML operations | Python/FastAPI | 8-16 GB RAM |
| **PostgreSQL** | Primary database, full-text search | PostgreSQL 17+ | 16-32 GB RAM |
| **Redis** | Caching, message queue | Redis 7+ | 1-2 GB RAM |
| **Caddy** | Load balancer, SSL termination | Caddy | 256 MB RAM |
| **Connectors** | Data source integration | Rust | 512 MB each |

## Deployment Models

### 1. Single Server (Recommended for \<1000 users)

**Requirements:**
- 8+ CPU cores
- 32+ GB RAM  
- 500+ GB SSD storage
- Docker + Docker Compose

**Deployment:**
```bash
git clone https://github.com/omnihq/omni.git
cd omni
docker compose up -d
```

### 2. Multi-Server (Enterprise)

**Architecture:**
- **Web Tier**: Load-balanced web servers
- **Application Tier**: Separate searcher/indexer instances
- **Data Tier**: Managed PostgreSQL + Redis cluster
- **AI Tier**: Dedicated GPU servers for ML workloads

**Deployment:**
- Use external managed databases (AWS RDS, Google Cloud SQL)
- Container orchestration (Kubernetes, Docker Swarm)
- Separate AI service deployment

### 3. Cloud Deployment

**Supported Platforms:**
- **AWS**: ECS, EKS, or EC2 instances
- **Google Cloud**: GKE, Compute Engine
- **Azure**: AKS, Container Instances
- **DigitalOcean**: App Platform, Droplets

## Security Model

### Authentication & Authorization

```
┌─────────────────┐    ┌─────────────────┐    ┌─────────────────┐
│  Identity       │    │      Omni       │    │  Data Sources   │
│  Provider       │    │                 │    │                 │
│                 │    │ ┌─────────────┐ │    │                 │
│ • Google OAuth  │◄──►│ │    RBAC     │ │◄──►│ • Google Drive  │
│ • SAML/OIDC     │    │ │   Engine    │ │    │ • Slack         │
│ • LDAP/AD       │    │ └─────────────┘ │    │ • Confluence    │
│                 │    │                 │    │                 │
└─────────────────┘    └─────────────────┘    └─────────────────┘
```

### Access Control
- **Authentication**: OAuth 2.0, SAML, OIDC integration
- **Authorization**: Role-based access control (RBAC)
- **Data Privacy**: Source-level permission inheritance
- **Audit Logging**: Complete activity tracking

### Network Security
- **TLS/SSL**: All external communications encrypted
- **Internal Network**: Isolated container networks
- **Firewall**: Configurable port restrictions
- **VPN Integration**: Support for corporate VPN requirements

## Performance Characteristics

### Scale Limits (Single Server)

| Metric | Limit | Notes |
|--------|-------|-------|
| **Documents** | 5M+ | With 32GB RAM, SSD storage |
| **Users** | 1000+ | Concurrent active users |
| **Search Latency** | \<500ms | 95th percentile |
| **Indexing Rate** | 1000/min | Documents per minute |
| **Storage Growth** | ~50MB/1K docs | Including embeddings |

### Optimization Features
- **Smart Caching**: Redis-based query caching
- **Index Optimization**: PostgreSQL FTS + pgvector
- **Async Processing**: Background document processing
- **Rate Limiting**: Configurable API rate limits

## Data Flow

### Document Indexing
```
Data Source → Connector → Queue → Indexer → AI Service → Database
     │                                          │
     └─ OAuth ────────────────────────────────────┘
```

### Search Processing
```
User → Web Interface → Searcher → Database → AI Service → Results
                           │                     │
                           └─ Cache ──────────────┘
```

## Getting Started Checklist

### Phase 1: Evaluation (1-2 hours)
- [ ] Review [System Requirements](./system-requirements)
- [ ] Deploy test instance using [Docker Guide](./docker-deployment)
- [ ] Connect one data source (Google Drive recommended)
- [ ] Test search functionality with sample documents
- [ ] Evaluate AI-powered answers

### Phase 2: Production Planning (1-2 days)
- [ ] Plan production architecture and sizing
- [ ] Obtain SSL certificates and configure DNS
- [ ] Set up monitoring and backup strategies
- [ ] Configure OAuth applications for data sources
- [ ] Plan user onboarding and training

### Phase 3: Production Deployment (1-2 weeks)
- [ ] Deploy production instance
- [ ] Configure all required data sources
- [ ] Set up monitoring and alerting
- [ ] Implement backup and disaster recovery
- [ ] User training and rollout

## Cost Analysis

### Infrastructure Costs (Monthly)

**Small Deployment** (500 users, 100K documents):
- Server: $200-400/month (cloud instance)
- Storage: $50-100/month
- Bandwidth: $20-50/month
- **Total**: $270-550/month

**Medium Deployment** (2000 users, 500K documents):
- Server: $800-1200/month
- Storage: $150-300/month  
- Bandwidth: $100-200/month
- **Total**: $1050-1700/month

**Enterprise Deployment** (10000+ users, 2M+ documents):
- Multi-server setup: $3000-6000/month
- Managed databases: $1000-2000/month
- Storage and bandwidth: $500-1000/month
- **Total**: $4500-9000/month

### vs. Commercial Solutions
- **Glean**: $25-50/user/month = $12,500-25,000/month (500 users)
- **Omni**: $270-550/month + setup time
- **ROI**: Break-even in 1-2 months for most deployments

## Support and Maintenance

### Community Support
- **GitHub Issues**: Bug reports and feature requests
- **Discussions**: Questions and community help
- **Documentation**: Comprehensive deployment guides

### Professional Services (Available)
- **Deployment Consulting**: Architecture and setup assistance
- **Custom Integrations**: Additional data source connectors
- **Enterprise Support**: SLA-backed support contracts
- **Training**: Administrator and user training programs

## Next Steps

Ready to deploy Omni? Choose your path:

**Quick Evaluation:**
→ [Docker Deployment](./docker-deployment) (10 minutes)

**Production Planning:**
→ [System Requirements](./system-requirements) → [Production Setup](../deployment/production-setup)

**Questions or Issues:**
→ [GitHub Discussions](https://github.com/omnihq/omni/discussions)