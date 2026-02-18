## Compute Module

This module creates ECS cluster, task definitions, and services for Omni.

## Resources Created

### Core Infrastructure
- ECS Fargate Cluster
- Service Discovery Private DNS Namespace
- IAM Roles (Task Execution, Task)

### Service Discovery
- 5 Service Discovery Services:
  - web
  - searcher
  - indexer
  - ai
  - google-connector

### Task Definitions
- migrator (one-time task)
- web (omni-web)
- searcher (omni-searcher)
- indexer (omni-indexer)
- ai (omni-ai)
- google-connector (omni-google-connector)

### ECS Services
- 5 long-running Fargate services
- Load balancer integration (web only)
- Service discovery integration
- ECS Exec enabled

## Usage

```hcl
module "compute" {
  source = "./modules/compute"

  customer_name     = "acme-corp"
  environment       = "production"
  github_org        = "omni-platform"

  vpc_id                = module.networking.vpc_id
  subnet_ids            = module.networking.private_subnet_ids
  security_group_id     = module.networking.ecs_security_group_id
  alb_target_group_arn  = module.loadbalancer.target_group_arn
  alb_dns_name          = module.loadbalancer.dns_name

  task_cpu        = "512"
  task_memory     = "1024"
  desired_count   = 1

  database_endpoint  = module.database.endpoint
  database_port      = module.database.port
  database_name      = module.database.database_name
  database_username  = var.database_username

  redis_endpoint = module.cache.endpoint
  redis_port     = module.cache.port

  log_group_name = module.monitoring.log_group_name
  region         = var.region

  database_password_arn = module.secrets.database_password_arn
  jina_api_key_arn      = module.secrets.jina_api_key_arn
  encryption_key_arn    = module.secrets.encryption_key_arn
  encryption_salt_arn   = module.secrets.encryption_salt_arn

  google_client_id     = var.google_client_id
  google_client_secret = var.google_client_secret
  resend_api_key       = var.resend_api_key
}
```

## Inputs

| Name | Description | Type | Default | Required |
|------|-------------|------|---------|----------|
| customer_name | Customer name for resource naming | string | - | yes |
| environment | Environment | string | "production" | no |
| github_org | GitHub organization for container images | string | - | yes |
| vpc_id | VPC ID | string | - | yes |
| subnet_ids | Private subnet IDs | list(string) | - | yes |
| security_group_id | ECS security group ID | string | - | yes |
| alb_target_group_arn | ALB target group ARN | string | - | yes |
| alb_dns_name | ALB DNS name | string | - | yes |
| task_cpu | ECS task CPU units | string | "512" | no |
| task_memory | ECS task memory (MB) | string | "1024" | no |
| desired_count | Desired tasks per service | number | 1 | no |
| database_endpoint | RDS endpoint | string | - | yes |
| database_port | RDS port | number | - | yes |
| database_name | Database name | string | - | yes |
| database_username | Database username | string | - | yes |
| redis_endpoint | Redis endpoint | string | - | yes |
| redis_port | Redis port | number | - | yes |
| log_group_name | CloudWatch log group | string | - | yes |
| region | AWS region | string | - | yes |
| database_password_arn | Database password secret ARN | string | - | yes |
| jina_api_key_arn | JINA API key secret ARN | string | - | yes |
| encryption_key_arn | Encryption key secret ARN | string | - | yes |
| encryption_salt_arn | Encryption salt secret ARN | string | - | yes |
| resend_api_key | Resend API key | string | "CONFIGURE_RESEND_API_KEY" | no |

## Outputs

| Name | Description |
|------|-------------|
| cluster_name | ECS cluster name |
| cluster_arn | ECS cluster ARN |
| web_service_name | Web service name |
| searcher_service_name | Searcher service name |
| indexer_service_name | Indexer service name |
| ai_service_name | AI service name |
| google_connector_service_name | Google connector service name |
| migrator_task_definition_arn | Migrator task definition ARN |
| task_execution_role_arn | Task execution role ARN |
| task_role_arn | Task role ARN |
| service_discovery_namespace_id | Service discovery namespace ID |

## Services

### Web Service (Port 3000)
- SvelteKit frontend and API
- Connected to ALB
- Accessible from internet via ALB

### Searcher Service (Port 3001)
- Search query processing
- Result ranking
- Internal service only

### Indexer Service (Port 3002)
- Document processing
- Database writes
- Internal service only

### AI Service (Port 3003)
- Embedding generation
- RAG orchestration
- Internal service only

### Google Connector Service (Port 3004)
- Google Workspace integration
- OAuth flows
- Webhook handling
- Internal service only

## Service Discovery

Services communicate via private DNS:
- `web.omni-{customer}.local:3000`
- `searcher.omni-{customer}.local:3001`
- `indexer.omni-{customer}.local:3002`
- `ai.omni-{customer}.local:3003`
- `google-connector.omni-{customer}.local:3004`

## Container Images

Images pulled from GitHub Container Registry:
- `ghcr.io/{github-org}/omni/omni-web:latest`
- `ghcr.io/{github-org}/omni/omni-searcher:latest`
- `ghcr.io/{github-org}/omni/omni-indexer:latest`
- `ghcr.io/{github-org}/omni/omni-ai:latest`
- `ghcr.io/{github-org}/omni/omni-google-connector:latest`
- `ghcr.io/{github-org}/omni/omni-migrator:latest`

Images must be public or authentication configured.

## IAM Roles

### Task Execution Role
- Pull container images
- Write CloudWatch logs
- Read secrets from Secrets Manager

### Task Role
- Invoke Bedrock models (for AI service)
- ECS Exec access (debugging)
- CloudWatch Logs access

## ECS Exec

Debug running containers:

```bash
aws ecs execute-command \
  --cluster omni-{customer}-cluster \
  --task <task-id> \
  --container omni-web \
  --interactive \
  --command "/bin/sh"
```

## Scaling

Update desired count:

```hcl
desired_count = 2
```

Or use auto-scaling (not configured by default).

## Resource Limits

Default per task:
- CPU: 512 units (0.5 vCPU)
- Memory: 1024 MB (1 GB)

Adjust based on workload:
- Development: 256 CPU / 512 MB
- Production: 1024 CPU / 2048 MB or higher
