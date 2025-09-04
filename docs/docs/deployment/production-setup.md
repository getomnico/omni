# Production Setup

This guide walks through deploying Omni for production use, including security hardening, SSL configuration, and performance optimization.

## Production Checklist

Before deploying to production, ensure you have:

- [ ] **System Requirements**: [Hardware and software requirements](../getting-started/system-requirements) met
- [ ] **Domain Name**: DNS configured to point to your server
- [ ] **SSL Certificates**: Valid certificates for HTTPS
- [ ] **Database**: Production PostgreSQL configuration
- [ ] **Backups**: Automated backup strategy implemented
- [ ] **Monitoring**: Health checks and alerting configured
- [ ] **Security**: Firewall, VPN, and access controls in place

## Production Configuration

### 1. Environment Variables

Create a production `.env` file:

```bash
# Production environment
NODE_ENV=production
RUST_LOG=info

# Database - Use strong passwords
DATABASE_URL=postgresql://omni:STRONG_PASSWORD_HERE@postgres:5432/omni

# Redis
REDIS_URL=redis://redis:6379

# Security - Generate secure random strings
JWT_SECRET=your-64-char-random-jwt-secret-here
ENCRYPTION_KEY=your-32-char-encryption-key-here

# External URLs
BASE_URL=https://search.yourcompany.com

# AI Service
VLLM_URL=http://vllm:8000

# OAuth Credentials (get from respective providers)
GOOGLE_CLIENT_ID=your-google-client-id
GOOGLE_CLIENT_SECRET=your-google-client-secret
SLACK_CLIENT_ID=your-slack-client-id
SLACK_CLIENT_SECRET=your-slack-client-secret
```

### 2. SSL/TLS Configuration

#### Option A: Let's Encrypt (Recommended)

Update `Caddyfile` for automatic SSL:

```caddyfile
search.yourcompany.com {
    reverse_proxy omni-web:3000
    
    # Security headers
    header {
        Strict-Transport-Security "max-age=31536000; includeSubDomains"
        X-Content-Type-Options "nosniff"
        X-Frame-Options "DENY"
        X-XSS-Protection "1; mode=block"
        Referrer-Policy "strict-origin-when-cross-origin"
    }
    
    # Enable compression
    encode gzip
    
    # Rate limiting
    rate_limit {
        zone dynamic_rate_limit {
            key    {remote_host}
            events 100
            window 1m
        }
    }
}
```

#### Option B: Custom Certificates

If using custom certificates, mount them:

```yaml
# In docker-compose.prod.yml
caddy:
  volumes:
    - ./ssl/cert.pem:/etc/ssl/cert.pem:ro
    - ./ssl/key.pem:/etc/ssl/key.pem:ro
    - ./Caddyfile.prod:/etc/caddy/Caddyfile:ro
```

Update `Caddyfile.prod`:

```caddyfile
search.yourcompany.com {
    tls /etc/ssl/cert.pem /etc/ssl/key.pem
    reverse_proxy omni-web:3000
}
```

### 3. Database Configuration

#### Production PostgreSQL Settings

Create `postgres/postgresql.conf`:

```ini
# Connection settings
max_connections = 200
shared_buffers = 8GB          # 25% of RAM
effective_cache_size = 24GB   # 75% of RAM
work_mem = 64MB
maintenance_work_mem = 2GB

# Write-ahead logging
wal_buffers = 64MB
checkpoint_completion_target = 0.9
max_wal_size = 4GB
min_wal_size = 1GB

# Query planner
random_page_cost = 1.1        # For SSD storage
effective_io_concurrency = 200

# Logging
log_statement = 'ddl'
log_checkpoints = on
log_connections = on
log_disconnections = on
log_lock_waits = on
```

#### Database Users and Permissions

```sql
-- Create read-only user for monitoring
CREATE USER omni_monitor WITH PASSWORD 'monitor_password';
GRANT CONNECT ON DATABASE omni TO omni_monitor;
GRANT USAGE ON SCHEMA public TO omni_monitor;
GRANT SELECT ON ALL TABLES IN SCHEMA public TO omni_monitor;

-- Create backup user
CREATE USER omni_backup WITH PASSWORD 'backup_password';
GRANT CONNECT ON DATABASE omni TO omni_backup;
GRANT USAGE ON SCHEMA public TO omni_backup;
GRANT SELECT ON ALL TABLES IN SCHEMA public TO omni_backup;
```

### 4. Resource Limits

Configure resource limits in `docker-compose.prod.yml`:

```yaml
services:
  omni-web:
    deploy:
      resources:
        limits:
          memory: 2G
          cpus: '1.0'
        reservations:
          memory: 1G
          cpus: '0.5'
    restart: unless-stopped

  omni-searcher:
    deploy:
      resources:
        limits:
          memory: 4G
          cpus: '2.0'
        reservations:
          memory: 2G
          cpus: '1.0'
    restart: unless-stopped

  omni-ai:
    deploy:
      resources:
        limits:
          memory: 16G
          cpus: '4.0'
        reservations:
          memory: 8G
          cpus: '2.0'
    restart: unless-stopped

  postgres:
    deploy:
      resources:
        limits:
          memory: 32G
          cpus: '8.0'
        reservations:
          memory: 16G
          cpus: '4.0'
    restart: unless-stopped
```

## Security Hardening

### 1. Network Security

#### Firewall Configuration

```bash
# Ubuntu/Debian with ufw
sudo ufw default deny incoming
sudo ufw default allow outgoing
sudo ufw allow ssh
sudo ufw allow 80/tcp
sudo ufw allow 443/tcp
sudo ufw enable

# RHEL/CentOS with firewalld
sudo firewall-cmd --permanent --add-service=ssh
sudo firewall-cmd --permanent --add-service=http
sudo firewall-cmd --permanent --add-service=https
sudo firewall-cmd --reload
```

#### Docker Network Isolation

Create isolated networks in `docker-compose.prod.yml`:

```yaml
networks:
  frontend:
    driver: bridge
  backend:
    driver: bridge
    internal: true  # No external access

services:
  caddy:
    networks:
      - frontend
      
  omni-web:
    networks:
      - frontend
      - backend
      
  postgres:
    networks:
      - backend  # Only internal access
```

### 2. Access Control

#### Administrative Access

```bash
# Create admin SSH key
ssh-keygen -t ed25519 -C "omni-admin@yourcompany.com"

# Disable password authentication
echo "PasswordAuthentication no" >> /etc/ssh/sshd_config
systemctl restart sshd
```

#### Application Access Control

Configure OAuth applications with restricted redirect URIs:
- **Google**: Only allow `https://search.yourcompany.com/api/oauth/google/callback`
- **Slack**: Only allow your production domain
- **GitHub**: Restrict to organization members only

### 3. Secrets Management

#### Using Docker Secrets

```yaml
# docker-compose.prod.yml
secrets:
  jwt_secret:
    file: ./secrets/jwt_secret.txt
  db_password:
    file: ./secrets/db_password.txt

services:
  omni-web:
    secrets:
      - jwt_secret
    environment:
      JWT_SECRET_FILE: /run/secrets/jwt_secret
```

#### Using External Secret Management

For enterprise environments, integrate with:
- **HashiCorp Vault**
- **AWS Secrets Manager**  
- **Azure Key Vault**
- **Kubernetes Secrets**

## Monitoring and Logging

### 1. Health Checks

Add comprehensive health checks:

```yaml
services:
  omni-web:
    healthcheck:
      test: ["CMD", "curl", "-f", "http://localhost:3000/api/health"]
      interval: 30s
      timeout: 10s
      retries: 3
      start_period: 60s
```

### 2. Log Management

#### Centralized Logging

```yaml
services:
  omni-web:
    logging:
      driver: "json-file"
      options:
        max-size: "100m"
        max-file: "10"
        tag: "omni-web"
```

#### Log Shipping

Consider shipping logs to:
- **ELK Stack** (Elasticsearch, Logstash, Kibana)
- **Prometheus + Grafana**
- **DataDog**
- **Splunk**

## Backup Strategy

### 1. Database Backups

#### Automated Daily Backups

```bash
#!/bin/bash
# backup-script.sh

BACKUP_DIR="/opt/omni/backups"
DATE=$(date +%Y%m%d_%H%M%S)
BACKUP_FILE="omni_backup_${DATE}.sql"

# Create backup
docker compose exec -T postgres pg_dump -U omni omni > "${BACKUP_DIR}/${BACKUP_FILE}"

# Compress backup
gzip "${BACKUP_DIR}/${BACKUP_FILE}"

# Keep only last 30 days
find ${BACKUP_DIR} -name "omni_backup_*.sql.gz" -mtime +30 -delete

# Upload to cloud storage (optional)
# aws s3 cp "${BACKUP_DIR}/${BACKUP_FILE}.gz" s3://your-backup-bucket/
```

#### Cron Job

```bash
# Add to crontab
0 2 * * * /opt/omni/backup-script.sh
```

### 2. Configuration Backups

```bash
# Backup configuration files
tar -czf /opt/omni/backups/config_$(date +%Y%m%d).tar.gz \
  /opt/omni/.env \
  /opt/omni/Caddyfile \
  /opt/omni/docker-compose.prod.yml
```

## Deployment

### 1. Production Deployment

```bash
# Clone repository
git clone https://github.com/omnihq/omni.git /opt/omni
cd /opt/omni

# Copy production configuration
cp .env.production .env
cp docker-compose.prod.yml.template docker-compose.prod.yml

# Start production stack
docker compose -f docker-compose.yml -f docker-compose.prod.yml up -d

# Verify deployment
docker compose ps
curl -f https://search.yourcompany.com/api/health
```

### 2. Rolling Updates

```bash
#!/bin/bash
# update-script.sh

cd /opt/omni

# Pull latest code
git pull origin main

# Rebuild and restart services one by one
services=("omni-web" "omni-searcher" "omni-indexer" "omni-ai")

for service in "${services[@]}"; do
    echo "Updating $service..."
    docker compose build $service
    docker compose up -d --no-deps $service
    
    # Wait for health check
    sleep 30
    
    # Verify service is healthy
    if ! docker compose ps $service | grep -q "healthy"; then
        echo "ERROR: $service failed to start properly"
        exit 1
    fi
done

echo "Update completed successfully"
```

## Performance Optimization

### 1. Database Optimization

```sql
-- Create additional indexes for performance
CREATE INDEX CONCURRENTLY idx_documents_source_created 
ON documents(source_id, created_at);

CREATE INDEX CONCURRENTLY idx_embeddings_document_similarity 
ON embeddings USING ivfflat (embedding vector_cosine_ops);

-- Update table statistics
ANALYZE;

-- Monitor slow queries
ALTER SYSTEM SET log_min_duration_statement = 1000;
SELECT pg_reload_conf();
```

### 2. Caching Strategy

Configure Redis for optimal performance:

```bash
# redis.conf
maxmemory 8gb
maxmemory-policy allkeys-lru
save 900 1
save 300 10
save 60 10000
```

### 3. AI Service Optimization

For better AI performance:

```yaml
omni-ai:
  environment:
    - VLLM_MAX_MODEL_LEN=4096
    - VLLM_TENSOR_PARALLEL_SIZE=2  # Use 2 GPUs if available
    - VLLM_DISABLE_LOG_STATS=true
```

## Next Steps

After production deployment:

1. **[Configure Monitoring](../operations/monitoring)** - Set up alerts and dashboards
2. **[Architecture Overview](../architecture/overview)** - Understand the system architecture
3. **[System Requirements](../getting-started/system-requirements)** - Review hardware and software needs