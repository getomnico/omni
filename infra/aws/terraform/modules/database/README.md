# Database Module

This module creates a ParadeDB instance running on ECS for Omni.

## Resources Created

- EC2 Auto Scaling Group for ParadeDB instances
- ECS Task Definition and Service for ParadeDB container
- ECS Capacity Provider for ParadeDB
- Security Group for ParadeDB
- IAM roles and policies for EC2 instances and ECS tasks
- CloudWatch Log Group for database logs
- Service Discovery entry for database DNS resolution
- EBS volumes for persistent PostgreSQL data

## Usage

```hcl
module "database" {
  source = "./modules/database"

  customer_name     = "acme-corp"
  environment       = "production"
  database_name     = "omni"
  database_username = "omni"

  # ParadeDB configuration
  paradedb_instance_type   = "t3.small"
  paradedb_volume_size     = 50
  paradedb_container_image = "paradedb/paradedb:0.20.6-pg17"

  # Infrastructure dependencies
  vpc_id                         = module.networking.vpc_id
  subnet_ids                     = module.networking.private_subnet_ids
  ecs_security_group_id          = module.networking.ecs_security_group_id
  ecs_cluster_name               = aws_ecs_cluster.main.name
  service_discovery_namespace_id = aws_service_discovery_private_dns_namespace.main.id
  database_password_secret_arn   = module.secrets.database_password_arn
  region                         = var.region
}
```

## Inputs

| Name | Description | Type | Default | Required |
|------|-------------|------|---------|----------|
| customer_name | Customer name for resource naming | string | - | yes |
| environment | Environment (production, staging, development) | string | "production" | no |
| database_name | PostgreSQL database name | string | "omni" | no |
| database_username | PostgreSQL master username | string | "omni" | no |
| paradedb_instance_type | EC2 instance type for ParadeDB | string | "t3.small" | no |
| paradedb_volume_size | EBS volume size in GB for ParadeDB data | number | 50 | no |
| paradedb_container_image | Docker image for ParadeDB | string | "paradedb/paradedb:0.20.6-pg17" | no |
| vpc_id | VPC ID for ParadeDB security group | string | - | yes |
| ecs_security_group_id | ECS security group ID to allow connections to ParadeDB | string | - | yes |
| ecs_cluster_name | ECS cluster name for ParadeDB service | string | - | yes |
| service_discovery_namespace_id | Service discovery namespace ID for ParadeDB | string | - | yes |
| database_password_secret_arn | ARN of the database password secret in Secrets Manager | string | - | yes |
| region | AWS region | string | - | yes |
| subnet_ids | List of subnet IDs for ParadeDB deployment | list(string) | - | yes |

## Outputs

| Name | Description |
|------|-------------|
| endpoint | ParadeDB endpoint address (via service discovery) |
| port | Database port (5432) |
| database_name | Database name |
| paradedb_capacity_provider_name | ParadeDB ECS capacity provider name |

## ParadeDB Configuration

### Engine Version
- ParadeDB 0.18.15 (based on PostgreSQL 17)
- Full-text search extensions built-in
- pgvector extension for semantic search

### Infrastructure
- Runs as an ECS service on dedicated EC2 instances
- Auto Scaling Group maintains exactly one instance
- Persistent EBS volume for database data (survives instance replacement)
- Private subnet deployment only
- Service discovery for DNS resolution (`paradedb.omni-{customer}.local`)

### Storage
- Type: gp3 (General Purpose SSD)
- Default size: 50 GB (configurable)
- Encrypted at rest
- Separate data volume mounted at `/var/lib/postgresql/data`
- Root volume: 30 GB for OS and container runtime

### Security
- Security group restricts access to ECS services only
- Password stored in AWS Secrets Manager
- Private subnet deployment
- IAM roles for EC2 instance and ECS tasks
- SSL/TLS support for connections

### Monitoring
- CloudWatch Logs: `/ecs/omni-{customer}/paradedb`
- Log retention: 7 days
- ECS Exec enabled for debugging
- Container health checks via pg_isready

## Scaling

ParadeDB runs on a single EC2 instance managed by an Auto Scaling Group for high availability.

### Vertical Scaling
To increase database capacity:

1. Update `paradedb_instance_type` (e.g., from `t3.small` to `t3.medium`)
2. Run `terraform apply`
3. The Auto Scaling Group will replace the instance with minimal downtime
4. Data persists on the EBS volume across instance replacements

### Storage Scaling
To increase storage:

1. Update `paradedb_volume_size`
2. Run `terraform apply`
3. Note: EBS volumes can only be increased, not decreased

## Cost Optimization

### Development/Staging
```hcl
paradedb_instance_type = "t3.small"
paradedb_volume_size   = 20
```

### Production
```hcl
paradedb_instance_type = "t3.medium"  # or larger
paradedb_volume_size   = 100
```

## High Availability

- Auto Scaling Group ensures instance is automatically replaced if it fails
- EBS volume persists data across instance replacements
- Service Discovery automatically updates DNS when instance is replaced
- Health checks ensure traffic only routes to healthy instances

## Backup and Recovery

Currently, backups are handled at the EBS volume level. Consider:
- EBS snapshots for point-in-time recovery
- Application-level backups using `pg_dump`
- Cross-region replication for disaster recovery

## Troubleshooting

### Connecting to the database
```bash
# Via ECS Exec
aws ecs execute-command \
  --cluster omni-{customer}-cluster \
  --task {task-id} \
  --container paradedb \
  --interactive \
  --command "/bin/bash"

# Then inside container
psql -U omni -d omni
```

### Viewing logs
```bash
aws logs tail /ecs/omni-{customer}/paradedb --follow
```

### Checking database health
The ECS task definition includes a health check that runs:
```bash
pg_isready -U omni -d omni
```

## Why ParadeDB?

ParadeDB extends PostgreSQL with:
- **Full-text search**: BM25 ranking built-in, no need for Elasticsearch
- **Hybrid search**: Combine keyword and semantic search
- **pgvector**: Native vector search for embeddings
- **Cost-effective**: Single database for all search needs
- **PostgreSQL compatibility**: Use standard PostgreSQL tools and clients
