---
sidebar_position: 1
---

# Omni Deployment Guide

**Enterprise AI Search Platform for IT Teams**

Omni is a self-hosted enterprise search platform that provides unified search across your organization's data sources with AI-powered answers. This documentation is designed to help IT teams deploy, configure, and maintain Omni in production environments.

## What is Omni?

Omni consolidates search across multiple enterprise data sources:
- **Google Workspace** (Drive, Docs, Gmail)
- **Slack** (messages, files, channels)  
- **Confluence** (pages, attachments, spaces)
- **GitHub** (repositories, issues, pull requests)
- **Local file systems**

Unlike cloud-based solutions like Glean, Omni runs entirely on your infrastructure, ensuring complete data privacy and control.

## Deployment Overview

Omni uses a **Docker Compose** architecture with these components:

| Service | Purpose | Technology |
|---------|---------|------------|
| **omni-web** | Frontend & API Gateway | SvelteKit/Node.js |
| **omni-searcher** | Search engine | Rust |
| **omni-indexer** | Document processing | Rust |  
| **omni-ai** | AI/ML services | Python/FastAPI |
| **PostgreSQL** | Primary database | PostgreSQL 17+ |
| **Redis** | Cache & message queue | Redis 7+ |
| **Caddy** | Load balancer & SSL | Caddy |
| **vLLM** | Local LLM inference | Python |

## Quick Deployment

Get Omni running in **under 10 minutes**:

```bash
# Clone the repository
git clone https://github.com/omnihq/omni.git
cd omni

# Start all services
docker compose up -d

# Access at https://localhost (or your domain)
```

→ **[Start here: Docker Deployment Guide](./getting-started/docker-deployment)**

## Production Checklist

Before deploying Omni in production:

- [ ] **System Requirements**: Verify hardware and software requirements
- [ ] **SSL Certificates**: Configure HTTPS with your certificates  
- [ ] **Database Setup**: Configure PostgreSQL with appropriate resources
- [ ] **Backup Strategy**: Set up automated database backups
- [ ] **Monitoring**: Configure health checks and alerting
- [ ] **Security**: Review authentication and network security
- [ ] **Data Sources**: Plan OAuth configurations for integrations

→ **[Production Setup Guide](./deployment/production-setup)**

## Architecture for IT Teams

Understanding Omni's architecture helps with:
- **Resource planning** and sizing
- **Network configuration** and security
- **Backup and disaster recovery** planning
- **Monitoring and troubleshooting**

→ **[Architecture Overview](./architecture/overview)**

## Support & Maintenance

- **Monitoring**: Health checks, metrics, and alerting
- **Updates**: Rolling updates and version management  
- **Backup/Restore**: Database and configuration backup strategies
- **Troubleshooting**: Common issues and debugging guides

→ **[Operations Guide](./operations/monitoring)**

## Getting Help

- **GitHub Issues**: [Report deployment issues](https://github.com/omnihq/omni/issues)
- **Discussions**: [Ask questions](https://github.com/omnihq/omni/discussions)
- **Documentation**: Search this documentation for specific topics

---

**Ready to deploy?** Start with the [System Requirements](./getting-started/system-requirements) to plan your deployment.
