# System Requirements

This page outlines the hardware and software requirements for deploying Omni in production environments.

## Hardware Requirements

### Minimum Requirements (Small Teams: \<1000 users, \<100K documents)

| Component | Specification |
|-----------|---------------|
| **CPU** | 4 cores (2.0+ GHz) |
| **RAM** | 8 GB |
| **Storage** | 100 GB SSD |
| **Network** | 1 Gbps |

### Recommended Requirements (Medium Teams: \<5000 users, \<1M documents)

| Component | Specification |
|-----------|---------------|
| **CPU** | 8 cores (3.0+ GHz) |
| **RAM** | 32 GB |
| **Storage** | 500 GB NVMe SSD |
| **Network** | 10 Gbps |

### Large Scale (Enterprise: \>5000 users, \>1M documents)

| Component | Specification |
|-----------|---------------|
| **CPU** | 16+ cores (3.0+ GHz) |
| **RAM** | 64+ GB |
| **Storage** | 1+ TB NVMe SSD |
| **Network** | 10+ Gbps |

## Software Requirements

### Operating System
- **Linux** (Ubuntu 22.04+ LTS, RHEL 8+, CentOS 8+)
- **Docker Support** required

### Required Software

| Software | Version | Purpose |
|----------|---------|---------|
| **Docker** | 24.0+ | Container runtime |
| **Docker Compose** | 2.20+ | Multi-container orchestration |
| **Git** | 2.30+ | Source code management |

### Network Requirements

#### Ports
The following ports need to be accessible:

| Port | Service | Purpose |
|------|---------|---------|
| **80** | HTTP | Redirect to HTTPS |
| **443** | HTTPS | Web interface |
| **5432** | PostgreSQL | Database (internal) |
| **6379** | Redis | Cache/Queue (internal) |

#### External Access
- **Outbound HTTPS (443)**: Required for OAuth and API integrations
- **DNS Resolution**: Required for external service authentication

## Storage Considerations

### Database Storage
- **PostgreSQL**: Grows with document count and embeddings
- **Estimate**: ~10-50 MB per 1000 documents
- **Backup Space**: Plan for 2x database size for backups

### Document Cache
- **Redis**: Stores search cache and message queue
- **Estimate**: ~1-5% of total document storage

### Log Storage
- **Application Logs**: ~100 MB per day (configurable)
- **Audit Logs**: ~50 MB per day per 1000 users

## Performance Planning

### CPU Usage
- **omni-ai**: Most CPU-intensive (ML operations)
- **omni-searcher**: Moderate CPU for search processing
- **PostgreSQL**: CPU scales with concurrent users

### Memory Usage
- **vLLM**: 8-16 GB (depends on model size)
- **PostgreSQL**: 4-8 GB (configurable)
- **Other services**: 2-4 GB combined

### I/O Patterns
- **Heavy Read**: Search operations
- **Moderate Write**: Document indexing
- **Sequential**: Log files and backups

## Scalability Considerations

### Horizontal Scaling
Currently, Omni runs on a single node. For larger deployments:
- Use external managed PostgreSQL (AWS RDS, Google Cloud SQL)
- Use external Redis cluster
- Run multiple searcher instances behind load balancer

### Vertical Scaling
- Add more CPU cores for AI processing
- Add more RAM for larger working sets
- Upgrade to faster storage for better I/O

## Security Requirements

### Network Security
- **Firewall**: Restrict access to internal ports
- **SSL/TLS**: HTTPS required for production
- **VPN**: Recommended for administrative access

### Data Security
- **Encryption at Rest**: Configure PostgreSQL encryption
- **Encryption in Transit**: All external communications use HTTPS
- **Backup Encryption**: Encrypt database backups

## Monitoring Requirements

### Health Checks
- Monitor all service containers
- Database connection health
- Disk space monitoring
- Memory usage alerts

### Metrics Collection
- Consider Prometheus + Grafana setup
- Log aggregation (ELK stack or similar)
- Application performance monitoring

## Next Steps

Once you've verified your system meets these requirements:

1. **[Docker Deployment](./docker-deployment)** - Quick deployment guide
2. **[Production Setup](../deployment/production-setup)** - Production configuration
3. **[Monitoring](../operations/monitoring)** - Operations and monitoring