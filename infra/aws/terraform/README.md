# Omni AWS Terraform Deployment

Complete Terraform infrastructure-as-code for deploying Omni to AWS.

## Overview

This Terraform configuration deploys a complete Omni installation in AWS, including:

- **Networking**: VPC, subnets, security groups, NAT gateway
- **Database**: RDS PostgreSQL 17 with pgvector
- **Cache**: ElastiCache Redis
- **Storage**: S3 buckets for content and batch inference
- **Compute**: ECS Fargate cluster with 5 services
- **Load Balancer**: Application Load Balancer with optional HTTPS
- **AI**: AWS Bedrock integration for embeddings and LLM
- **Monitoring**: CloudWatch Logs with optional OpenTelemetry
- **Secrets**: AWS Secrets Manager for credentials
- **Migrations**: Automatic database migrations

## Prerequisites

### Required Software

- [Terraform](https://www.terraform.io/downloads) >= 1.5.0
- [AWS CLI](https://aws.amazon.com/cli/) >= 2.0
- `jq` (for scripts)

### AWS Account Setup

1. **Create a dedicated AWS account** for Omni (recommended)
   - Via AWS Organizations or standalone account
   - Ensures isolation and cost tracking

2. **Configure AWS credentials**
   ```bash
   aws configure
   # or export AWS_ACCESS_KEY_ID, AWS_SECRET_ACCESS_KEY
   ```

3. **Verify access**
   ```bash
   aws sts get-caller-identity
   ```

### Required Information

Before deployment, gather:

- **Customer name**: Unique identifier (e.g., `acme-corp`)
- **GitHub organization**: For container images (use `getomnico`)
- **JINA API key**: Get from [jina.ai](https://jina.ai/)
- **Custom domain**: Domain name for your deployment (e.g., `demo.getomni.co`)
- **Google OAuth credentials**: For Google Workspace integration (optional)
- **Resend API key**: For email functionality (optional)
- **SSL certificate ARN**: For HTTPS (optional)

## Quick Start

### 1. Clone Repository

```bash
git clone https://github.com/your-org/omni.git
cd omni/infra/aws/terraform
```

### 2. Initialize Backend (Recommended)

For production deployments, use remote state:

```bash
./scripts/init-backend.sh
cp backend.tf.example backend.tf

# Update backend.tf with your account ID
ACCOUNT_ID=$(aws sts get-caller-identity --query Account --output text)
sed -i "s/{account-id}/$ACCOUNT_ID/g" backend.tf
```

### 3. Configure Variables

```bash
cp terraform.tfvars.example terraform.tfvars
```

Edit `terraform.tfvars` with your values:

```hcl
# Required
customer_name = "acme-corp"
github_org    = "omni-platform"
jina_api_key  = "your-jina-api-key"
custom_domain = "demo.getomni.co"

# Optional
region      = "us-east-1"
environment = "production"

# Configure Google OAuth
google_client_id     = "your-client-id.apps.googleusercontent.com"
google_client_secret = "your-client-secret"

# Configure email
resend_api_key = "re_your_api_key"
```

### 4. Deploy

```bash
# Option 1: Interactive deployment
./scripts/deploy.sh

# Option 2: Plan only (preview changes)
./scripts/deploy.sh --plan

# Option 3: Auto-approve (CI/CD)
./scripts/deploy.sh --yes
```

### 5. Access Your Application

After deployment completes:

```bash
terraform output omni_url
# Output: http://omni-acme-corp-alb-1234567890.us-east-1.elb.amazonaws.com
```

Visit the URL to access Omni!

## Directory Structure

```
terraform/
├── main.tf                   # Main configuration
├── variables.tf              # Input variables
├── outputs.tf                # Output values
├── versions.tf               # Provider versions
├── terraform.tfvars.example  # Example variables
├── backend.tf.example        # Backend configuration
├── modules/                  # Terraform modules
│   ├── networking/           # VPC, subnets, security groups
│   ├── database/             # RDS PostgreSQL
│   ├── cache/                # ElastiCache Redis
│   ├── storage/              # S3 buckets (content, batch inference)
│   ├── monitoring/           # CloudWatch Logs
│   ├── loadbalancer/         # Application Load Balancer
│   ├── compute/              # ECS cluster and services
│   ├── secrets/              # AWS Secrets Manager
│   └── migrations/           # Database migrations
├── scripts/                  # Deployment scripts
│   ├── init-backend.sh       # Initialize S3 backend
│   ├── validate.sh           # Validate configuration
│   └── deploy.sh             # Deploy infrastructure
└── README.md                 # This file
```

## Configuration

### Required Variables

| Variable | Description | Example |
|----------|-------------|---------|
| `customer_name` | Customer identifier | `acme-corp` |
| `github_org` | GitHub org for images | `omni-platform` |
| `jina_api_key` | JINA AI API key | `jina_xxx` |
| `custom_domain` | Custom domain name | `demo.getomni.co` |

### Optional Variables

#### AWS Configuration
- `region`: AWS region (default: `us-east-1`)
- `environment`: Environment type (default: `production`)

#### Database
- `db_instance_class`: RDS instance type (default: `db.t3.micro`)
- `db_allocated_storage`: Storage in GB (default: `20`)
- `db_multi_az`: Enable Multi-AZ (default: `false`)
- `skip_final_snapshot`: Skip final snapshot (default: `false`)

#### Cache
- `redis_node_type`: Redis node type (default: `cache.t3.micro`)
- `redis_engine_version`: Redis version (default: `7.1`)

#### ECS
- `ecs_task_cpu`: Task CPU units (default: `512`)
- `ecs_task_memory`: Task memory in MB (default: `1024`)
- `ecs_desired_count`: Tasks per service (default: `1`)

#### Load Balancer
- `ssl_certificate_arn`: ACM certificate ARN for HTTPS
- `alb_deletion_protection`: Enable deletion protection (default: `false`)

#### Application
- `google_client_id`: Google OAuth client ID
- `google_client_secret`: Google OAuth client secret
- `resend_api_key`: Resend API key for emails

See `variables.tf` for the complete list.

## Infrastructure Components

### Networking

- **VPC**: 10.0.0.0/16 with DNS enabled
- **Public Subnets**: 2 AZs for ALB (10.0.1.0/24, 10.0.2.0/24)
- **Private Subnets**: 2 AZs for services (10.0.11.0/24, 10.0.12.0/24)
- **NAT Gateway**: Outbound internet for private subnets
- **Security Groups**: Layered security (ALB, ECS, RDS, Redis)

### Database

- **Engine**: PostgreSQL 17.2
- **Extensions**: pgvector for embeddings
- **Storage**: gp3 SSD, encrypted at rest
- **Backups**: 7-day retention (configurable)
- **SSL**: Required for all connections

### Cache

- **Engine**: Redis 7.1
- **Deployment**: Single node (Multi-AZ optional)
- **Purpose**: Sessions, caching

### Storage

- **Content Bucket**: S3 bucket for document content storage
  - Versioning enabled
  - Server-side encryption (AES256)
  - Public access blocked
- **Batch Bucket**: S3 bucket for Bedrock batch inference
  - Server-side encryption (AES256)
  - 7-day lifecycle policy for cleanup
  - IAM role for Bedrock service access

### Compute

- **ECS Cluster**: Fargate capacity provider with Container Insights
- **Services**:
  - `omni-web`: SvelteKit frontend and API (port 3000)
  - `omni-searcher`: Search service with typo tolerance (port 3001)
  - `omni-indexer`: Document processing and indexing (port 3002)
  - `omni-ai`: AI service with Bedrock integration (port 3003)
  - `omni-google-connector`: Google Workspace integration (port 3004)
- **Service Discovery**: AWS Cloud Map private DNS namespace
- **Container Images**: GitHub Container Registry (GHCR)
- **Storage Integration**: All services configured with S3 backend
- **Observability**: Optional OpenTelemetry integration

### Load Balancer

- **Type**: Application Load Balancer
- **Listeners**: HTTP (80), HTTPS (443, optional)
- **Target**: ECS web service
- **Health Check**: `/health` endpoint

### Monitoring

- **CloudWatch Logs**: `/ecs/omni-{customer}`
- **Retention**: 30 days (configurable)
- **Container Insights**: Enabled on ECS cluster

### AI Integration

- **Embedding Provider**: JINA AI (via API)
- **LLM Provider**: AWS Bedrock
  - Default model: Amazon Nova Pro (RAG)
  - Title generation: Amazon Nova Lite
  - Alternative: Anthropic Claude Sonnet/Haiku (commented in config)
- **Batch Processing**: Optional Bedrock batch inference
  - Configurable batch size and timeout
  - S3-based job management
  - Currently disabled (requires AWS support case)

### Secrets

All secrets stored in AWS Secrets Manager:
- Database password (auto-generated)
- JINA API key
- Encryption keys (auto-generated)
- Encryption salt (auto-generated)
- Session secret (auto-generated)

## Post-Deployment

### Setup HTTPS

1. **Request ACM Certificate**
   ```bash
   aws acm request-certificate \
     --domain-name omni.your-domain.com \
     --validation-method DNS \
     --region us-east-1
   ```

2. **Validate Certificate**
   - Add DNS records shown in ACM console
   - Wait for validation

3. **Update Configuration**
   ```hcl
   # terraform.tfvars
   ssl_certificate_arn = "arn:aws:acm:us-east-1:123456789012:certificate/..."
   ```

4. **Apply Changes**
   ```bash
   ./scripts/deploy.sh
   ```

5. **Update DNS**
   ```bash
   # Get ALB DNS
   terraform output alb_dns_name

   # Create CNAME or ALIAS record
   omni.your-domain.com -> omni-customer-alb-xxx.elb.amazonaws.com
   ```

### Configure Integrations

#### Google Workspace

1. Create OAuth 2.0 credentials in [Google Cloud Console](https://console.cloud.google.com)
2. Add authorized redirect URI: `https://omni.your-domain.com/auth/google/callback`
3. Update `terraform.tfvars`:
   ```hcl
   google_client_id     = "xxx.apps.googleusercontent.com"
   google_client_secret = "GOCSPX-xxx"
   ```
4. Re-deploy: `./scripts/deploy.sh`

#### Email (Resend)

1. Sign up at [resend.com](https://resend.com)
2. Create API key
3. Update `terraform.tfvars`:
   ```hcl
   resend_api_key = "re_xxx"
   ```
4. Re-deploy: `./scripts/deploy.sh`

### Monitor Deployment

#### View Logs

```bash
# All services
aws logs tail $(terraform output -raw log_group_name) --follow

# Specific service
aws logs tail $(terraform output -raw log_group_name) \
  --follow \
  --filter-pattern "web"
```

#### Check Service Health

```bash
# ECS services
aws ecs describe-services \
  --cluster $(terraform output -raw ecs_cluster_name) \
  --services omni-customer-web

# ALB target health
aws elbv2 describe-target-health \
  --target-group-arn $(terraform output -json | jq -r '.alb_target_group_arn.value')
```

#### Debug Container

```bash
# Get running task ID
TASK_ID=$(aws ecs list-tasks \
  --cluster $(terraform output -raw ecs_cluster_name) \
  --service-name omni-customer-web \
  --query 'taskArns[0]' \
  --output text)

# Connect to container
aws ecs execute-command \
  --cluster $(terraform output -raw ecs_cluster_name) \
  --task $TASK_ID \
  --container omni-web \
  --interactive \
  --command "/bin/sh"
```

## Scaling

### Vertical Scaling (Increase Resources)

Update `terraform.tfvars`:

```hcl
# Database
db_instance_class = "db.t3.small"  # or db.t3.medium, db.r6g.large, etc.

# Cache
redis_node_type = "cache.t3.small"

# ECS Tasks
ecs_task_cpu    = "1024"  # 1 vCPU
ecs_task_memory = "2048"  # 2 GB
```

Apply changes:
```bash
./scripts/deploy.sh
```

### Horizontal Scaling (More Instances)

Update desired count:

```hcl
ecs_desired_count = 2  # Run 2 instances of each service
```

Or use auto-scaling (not configured by default):

```hcl
# In modules/compute/services.tf
resource "aws_appautoscaling_target" "ecs" {
  # Add auto-scaling configuration
}
```

### Multi-AZ Database

Enable for production:

```hcl
db_multi_az = true
```

## Maintenance

### Updating Services

Update container images:

```bash
# Services automatically pull :latest on deployment
# Force new deployment to pull updated images
aws ecs update-service \
  --cluster $(terraform output -raw ecs_cluster_name) \
  --service omni-customer-web \
  --force-new-deployment
```

### Database Backups

Automated backups are enabled by default (7-day retention).

Create manual snapshot:

```bash
aws rds create-db-snapshot \
  --db-instance-identifier omni-customer-postgres \
  --db-snapshot-identifier omni-customer-manual-$(date +%Y%m%d)
```

### Terraform State

State is stored in S3 with versioning enabled.

List state versions:

```bash
ACCOUNT_ID=$(aws sts get-caller-identity --query Account --output text)
aws s3api list-object-versions \
  --bucket omni-terraform-state-$ACCOUNT_ID \
  --prefix omni/terraform.tfstate
```

## Troubleshooting

### Deployment Fails

**Issue**: Terraform apply fails

**Solutions**:
1. Check error message carefully
2. Verify AWS credentials: `aws sts get-caller-identity`
3. Check service quotas: AWS Console → Service Quotas
4. Review CloudFormation events if using modules

### Services Won't Start

**Issue**: ECS tasks fail to start

**Solutions**:
1. Check CloudWatch Logs: `aws logs tail /ecs/omni-customer --follow`
2. Verify container images are accessible
3. Check security groups allow required traffic
4. Verify database and Redis are accessible

### Database Connection Errors

**Issue**: Services can't connect to database

**Solutions**:
1. Verify security group allows port 5432 from ECS
2. Check database endpoint: `terraform output database_endpoint`
3. Test connectivity from ECS task:
   ```bash
   aws ecs execute-command \
     --cluster ... \
     --task ... \
     --container omni-web \
     --interactive \
     --command "nc -zv database-endpoint 5432"
   ```

### Migrations Failed

**Issue**: Database migrations don't run

**Solutions**:
1. Check migration Lambda logs:
   ```bash
   aws logs tail /aws/lambda/omni-customer-migrator --follow
   ```
2. Manually run migrations:
   ```bash
   aws ecs run-task \
     --cluster omni-customer-cluster \
     --task-definition omni-customer-migrator \
     --launch-type FARGATE \
     --network-configuration "..."
   ```

### High Costs

**Issue**: Monthly costs exceeding budget

**Solutions**:
1. Check Cost Explorer for breakdown
2. Reduce instance sizes for dev environments
3. Use Fargate Spot for non-critical workloads
4. Consider NAT instance instead of NAT Gateway for dev

## Destroying Infrastructure

**⚠️ WARNING**: This will delete all data permanently!

### Backup First

```bash
# Database snapshot
aws rds create-db-snapshot \
  --db-instance-identifier omni-customer-postgres \
  --db-snapshot-identifier omni-customer-final-$(date +%Y%m%d)

# Wait for completion
aws rds wait db-snapshot-completed \
  --db-snapshot-identifier omni-customer-final-$(date +%Y%m%d)
```

### Destroy

```bash
# Interactive (asks for confirmation)
./scripts/deploy.sh --destroy

# Auto-approve
./scripts/deploy.sh --destroy --yes

# Or directly
terraform destroy
```

### Clean Up

```bash
# Remove backend resources (optional)
ACCOUNT_ID=$(aws sts get-caller-identity --query Account --output text)
aws s3 rb s3://omni-terraform-state-$ACCOUNT_ID --force
aws dynamodb delete-table --table-name omni-terraform-locks
```

## CI/CD Integration

### GitHub Actions Example

```yaml
name: Deploy Omni

on:
  push:
    branches: [main]

jobs:
  deploy:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v3

      - uses: hashicorp/setup-terraform@v2
        with:
          terraform_version: 1.5.0

      - name: Configure AWS
        uses: aws-actions/configure-aws-credentials@v2
        with:
          aws-access-key-id: ${{ secrets.AWS_ACCESS_KEY_ID }}
          aws-secret-access-key: ${{ secrets.AWS_SECRET_ACCESS_KEY }}
          aws-region: us-east-1

      - name: Deploy
        working-directory: infra/aws/terraform
        run: |
          terraform init
          terraform plan -out=tfplan
          terraform apply tfplan
```

## Support

### Documentation

- [Terraform AWS Provider](https://registry.terraform.io/providers/hashicorp/aws/latest/docs)
- [AWS ECS Documentation](https://docs.aws.amazon.com/ecs/)
- [Omni Architecture](../../../CLAUDE.md)

### Issues

Report issues at: https://github.com/your-org/omni/issues

### Community

Join discussions: [Discord/Slack link]

## License

[Your License]
