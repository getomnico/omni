# Monitoring and Alerting

Effective monitoring is crucial for maintaining Omni in production. This guide covers health checks, metrics collection, alerting, and troubleshooting.

## Health Check Overview

Omni includes built-in health checks for all services. Monitor these endpoints to ensure system health:

| Service | Health Check URL | Purpose |
|---------|------------------|---------|
| **Web Interface** | `https://your-domain.com/api/health` | Overall system status |
| **Searcher** | `http://omni-searcher:8080/health` | Search service health |
| **Indexer** | `http://omni-indexer:8081/health` | Indexing service health |
| **AI Service** | `http://omni-ai:8000/health` | AI/ML service health |
| **Database** | PostgreSQL connection check | Database connectivity |
| **Redis** | Redis ping | Cache/queue health |

## Basic Monitoring Setup

### 1. Docker Health Checks

All services include Docker health checks. View status:

```bash
# Check all services
docker compose ps

# Monitor specific service
watch docker compose ps omni-searcher
```

### 2. Health Check Script

Create a simple monitoring script:

```bash
#!/bin/bash
# health-check.sh

DOMAIN="https://search.yourcompany.com"
LOG_FILE="/var/log/omni-health.log"

check_service() {
    local service=$1
    local url=$2
    
    if curl -f -s "$url" > /dev/null; then
        echo "$(date): $service - OK" >> $LOG_FILE
        return 0
    else
        echo "$(date): $service - FAILED" >> $LOG_FILE
        return 1
    fi
}

# Check main health endpoint
if ! check_service "Main" "$DOMAIN/api/health"; then
    # Send alert (email, Slack, etc.)
    echo "Omni health check failed" | mail -s "Omni Alert" admin@yourcompany.com
fi
```

### 3. Cron-based Monitoring

```bash
# Add to crontab for every 5 minutes
*/5 * * * * /opt/omni/scripts/health-check.sh
```

## Advanced Monitoring with Prometheus

### 1. Prometheus Configuration

Create `prometheus.yml`:

```yaml
global:
  scrape_interval: 15s
  evaluation_interval: 15s

scrape_configs:
  - job_name: 'omni-web'
    static_configs:
      - targets: ['omni-web:3000']
    metrics_path: '/api/metrics'
    
  - job_name: 'omni-searcher'
    static_configs:
      - targets: ['omni-searcher:8080']
    metrics_path: '/metrics'
    
  - job_name: 'postgres'
    static_configs:
      - targets: ['postgres-exporter:9187']
      
  - job_name: 'redis'
    static_configs:
      - targets: ['redis-exporter:9121']
      
  - job_name: 'cadvisor'
    static_configs:
      - targets: ['cadvisor:8080']
```

### 2. Docker Compose for Monitoring Stack

Add to `docker-compose.monitoring.yml`:

```yaml
version: '3.8'

services:
  prometheus:
    image: prom/prometheus:latest
    container_name: omni-prometheus
    ports:
      - "9090:9090"
    volumes:
      - ./monitoring/prometheus.yml:/etc/prometheus/prometheus.yml
      - prometheus_data:/prometheus
    command:
      - '--config.file=/etc/prometheus/prometheus.yml'
      - '--storage.tsdb.path=/prometheus'
      - '--web.console.libraries=/etc/prometheus/console_libraries'
      - '--web.console.templates=/etc/prometheus/consoles'
      - '--storage.tsdb.retention.time=30d'
      - '--web.enable-lifecycle'

  grafana:
    image: grafana/grafana:latest
    container_name: omni-grafana
    ports:
      - "3001:3000"
    environment:
      - GF_SECURITY_ADMIN_PASSWORD=admin
    volumes:
      - grafana_data:/var/lib/grafana
      - ./monitoring/grafana/dashboards:/etc/grafana/provisioning/dashboards
      - ./monitoring/grafana/datasources:/etc/grafana/provisioning/datasources

  postgres-exporter:
    image: prometheuscommunity/postgres-exporter:latest
    container_name: omni-postgres-exporter
    environment:
      DATA_SOURCE_NAME: "postgresql://omni_monitor:monitor_password@postgres:5432/omni?sslmode=disable"
    ports:
      - "9187:9187"

  redis-exporter:
    image: oliver006/redis_exporter:latest
    container_name: omni-redis-exporter
    environment:
      REDIS_ADDR: "redis://redis:6379"
    ports:
      - "9121:9121"

  cadvisor:
    image: gcr.io/cadvisor/cadvisor:latest
    container_name: omni-cadvisor
    ports:
      - "8080:8080"
    volumes:
      - /:/rootfs:ro
      - /var/run:/var/run:ro
      - /sys:/sys:ro
      - /var/lib/docker/:/var/lib/docker:ro
      - /dev/disk/:/dev/disk:ro
    privileged: true
    devices:
      - /dev/kmsg

volumes:
  prometheus_data:
  grafana_data:
```

### 3. Start Monitoring Stack

```bash
# Start monitoring alongside Omni
docker compose -f docker-compose.yml -f docker-compose.monitoring.yml up -d

# Access Grafana at http://localhost:3001
# Access Prometheus at http://localhost:9090
```

## Key Metrics to Monitor

### 1. System Metrics

| Metric | Description | Alert Threshold |
|--------|-------------|-----------------|
| **CPU Usage** | Per-service CPU utilization | \>80% for 5 minutes |
| **Memory Usage** | Per-service memory utilization | \>90% for 2 minutes |
| **Disk Usage** | Storage space utilization | \>85% |
| **Network I/O** | Network traffic patterns | Unusual spikes |

### 2. Application Metrics

| Metric | Description | Alert Threshold |
|--------|-------------|-----------------|
| **Search Latency** | Time to process search queries | \>2 seconds |
| **Indexing Rate** | Documents processed per minute | \<100/min |
| **Error Rate** | HTTP 5xx responses | \>5% |
| **Queue Length** | Pending indexing jobs | \>10,000 |

### 3. Database Metrics

| Metric | Description | Alert Threshold |
|--------|-------------|-----------------|
| **Connection Count** | Active database connections | \>80% of max |
| **Query Duration** | Slow query detection | \>5 seconds |
| **Lock Waits** | Database lock contention | \>100 waits/min |
| **Cache Hit Ratio** | PostgreSQL buffer cache efficiency | \<95% |

### 4. AI Service Metrics

| Metric | Description | Alert Threshold |
|--------|-------------|-----------------|
| **Model Inference Time** | Time to generate embeddings | \>10 seconds |
| **GPU Utilization** | GPU usage (if applicable) | \>95% |
| **Memory Usage** | AI service memory consumption | \>90% |
| **Queue Depth** | Pending AI requests | \>50 |

## Alerting Configuration

### 1. Prometheus Alerting Rules

Create `alerts.yml`:

```yaml
groups:
  - name: omni-alerts
    rules:
      - alert: ServiceDown
        expr: up == 0
        for: 1m
        labels:
          severity: critical
        annotations:
          summary: "Service {{ $labels.job }} is down"
          description: "Service {{ $labels.job }} has been down for more than 1 minute"

      - alert: HighCPUUsage
        expr: rate(container_cpu_usage_seconds_total[5m]) * 100 \> 80
        for: 5m
        labels:
          severity: warning
        annotations:
          summary: "High CPU usage on {{ $labels.name }}"

      - alert: HighMemoryUsage
        expr: container_memory_usage_bytes / container_spec_memory_limit_bytes * 100 \> 90
        for: 2m
        labels:
          severity: critical
        annotations:
          summary: "High memory usage on {{ $labels.name }}"

      - alert: HighDiskUsage
        expr: (node_filesystem_size_bytes - node_filesystem_avail_bytes) / node_filesystem_size_bytes * 100 \> 85
        for: 1m
        labels:
          severity: warning
        annotations:
          summary: "High disk usage on {{ $labels.device }}"

      - alert: SearchLatencyHigh
        expr: histogram_quantile(0.95, rate(http_request_duration_seconds_bucket{job="omni-searcher"}[5m])) \> 2
        for: 3m
        labels:
          severity: warning
        annotations:
          summary: "High search latency detected"

      - alert: DatabaseConnections
        expr: pg_stat_database_numbackends / pg_settings_max_connections * 100 \> 80
        for: 2m
        labels:
          severity: warning
        annotations:
          summary: "High database connection usage"
```

### 2. Alertmanager Configuration

Create `alertmanager.yml`:

```yaml
global:
  smtp_smarthost: 'localhost:587'
  smtp_from: 'alerts@yourcompany.com'

route:
  group_by: ['alertname']
  group_wait: 10s
  group_interval: 10s
  repeat_interval: 1h
  receiver: 'web.hook'

receivers:
  - name: 'web.hook'
    email_configs:
      - to: 'admin@yourcompany.com'
        subject: 'Omni Alert: {{ .GroupLabels.alertname }}'
        body: |
          {{ range .Alerts }}
          Alert: {{ .Annotations.summary }}
          Description: {{ .Annotations.description }}
          Labels: {{ .Labels }}
          {{ end }}
    
    slack_configs:
      - api_url: 'YOUR_SLACK_WEBHOOK_URL'
        channel: '#alerts'
        title: 'Omni Alert'
        text: '{{ range .Alerts }}{{ .Annotations.summary }}{{ end }}'
```

## Log Management

### 1. Centralized Logging

Configure log shipping to centralized systems:

```yaml
# docker-compose.yml additions
services:
  omni-web:
    logging:
      driver: "fluentd"
      options:
        fluentd-address: "fluentd:24224"
        tag: "omni.web"
```

### 2. Log Analysis Queries

Common log analysis patterns:

```bash
# Search for errors in the last hour
docker compose logs --since 1h omni-web | grep -i error

# Monitor slow queries
docker compose logs postgres | grep "duration"

# Check authentication failures
docker compose logs omni-web | grep "authentication failed"

# Monitor indexing progress
docker compose logs omni-indexer | grep "processed"
```

### 3. Log Rotation

Configure log rotation to prevent disk space issues:

```bash
# /etc/logrotate.d/omni
/var/lib/docker/containers/*/*-json.log {
    daily
    rotate 7
    compress
    delaycompress
    missingok
    notifempty
    create 0644 root root
    postrotate
        docker kill --signal=USR1 $(docker ps -q) 2>/dev/null || true
    endscript
}
```

## Performance Monitoring

### 1. Database Performance

Monitor key database metrics:

```sql
-- Active connections
SELECT count(*) FROM pg_stat_activity;

-- Long-running queries
SELECT 
  now() - query_start as duration,
  query 
FROM pg_stat_activity 
WHERE query != '\<IDLE>' 
ORDER BY duration DESC;

-- Index usage
SELECT 
  schemaname,
  tablename,
  attname,
  n_distinct,
  correlation 
FROM pg_stats 
WHERE schemaname = 'public';

-- Cache hit ratio
SELECT 
  'buffer_cache' as metric,
  (sum(heap_blks_hit) / (sum(heap_blks_hit) + sum(heap_blks_read))) * 100 as hit_ratio
FROM pg_statio_user_tables;
```

### 2. Search Performance

Track search performance metrics:

```bash
# Average search response time
docker compose logs omni-searcher | grep "search_duration" | tail -100

# Search query distribution
docker compose logs omni-searcher | grep "query:" | cut -d'"' -f4 | sort | uniq -c | sort -nr
```

### 3. AI Service Performance

Monitor AI service performance:

```bash
# Embedding generation time
docker compose logs omni-ai | grep "embedding_duration"

# Model loading time
docker compose logs omni-ai | grep "model_load_time"

# GPU utilization (if applicable)
nvidia-smi --query-gpu=utilization.gpu --format=csv,noheader,nounits
```

## Troubleshooting Guide

### 1. Service Won't Start

```bash
# Check service logs
docker compose logs \<service-name>

# Check resource constraints
docker stats

# Verify network connectivity
docker compose exec omni-web ping omni-searcher
```

### 2. Performance Issues

```bash
# Check system resources
htop
iostat -x 1
free -h

# Check database performance
docker compose exec postgres pg_stat_activity

# Monitor network
netstat -i
ss -tuln
```

### 3. Search Issues

```bash
# Check indexer status
docker compose logs omni-indexer | tail -50

# Verify database connections
docker compose exec postgres psql -U omni -c "SELECT count(*) FROM documents;"

# Test search directly
curl -X POST http://localhost/api/search -d '{"query": "test"}'
```

## Dashboard Templates

### 1. Grafana Dashboard JSON

Create comprehensive dashboards for:
- **System Overview**: CPU, memory, disk, network
- **Application Metrics**: Search latency, indexing rate, error rates
- **Database Performance**: Connections, query time, cache hit ratio
- **User Activity**: Search volume, popular queries, user sessions

### 2. Custom Metrics

Add custom application metrics:

```rust
// In Rust services
use prometheus::{Counter, Histogram, register_counter, register_histogram};

lazy_static! {
    static ref SEARCH_REQUESTS: Counter = register_counter!(
        "search_requests_total", "Total number of search requests"
    ).unwrap();
    
    static ref SEARCH_DURATION: Histogram = register_histogram!(
        "search_duration_seconds", "Search request duration"
    ).unwrap();
}
```

## Next Steps

After setting up monitoring:

1. **[Production Setup](../deployment/production-setup)** - Production deployment guide
2. **[Architecture Overview](../architecture/overview)** - Understanding system components
3. **[Docker Deployment](../getting-started/docker-deployment)** - Quick deployment guide