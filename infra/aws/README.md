# Omni AWS Infrastructure

Terraform infrastructure-as-code for deploying Omni to AWS. Provisions a complete environment including networking, compute (ECS Fargate), database (RDS PostgreSQL), caching (ElastiCache Redis), and supporting services.

## Directory Structure

```
infra/aws/
├── terraform/
│   ├── main.tf                     # Root module - wires everything together
│   ├── variables.tf                # Input variables
│   ├── terraform.tfvars.example    # Example variable values
│   ├── scripts/
│   │   ├── deploy.sh               # Deployment script
│   │   ├── init-backend.sh         # S3 backend initialization
│   │   └── validate.sh             # Pre-deploy validation
│   └── modules/
│       ├── networking/             # VPC, subnets, NAT gateway, security groups
│       ├── database/               # RDS PostgreSQL with ParadeDB
│       ├── compute/                # ECS Fargate cluster, services, task definitions
│       ├── cache/                  # ElastiCache Redis
│       ├── storage/                # S3 buckets
│       ├── loadbalancer/           # ALB, target groups, listeners
│       ├── secrets/                # AWS Secrets Manager
│       ├── monitoring/             # CloudWatch log groups and alarms
│       └── migrations/             # Lambda-based database migration runner
├── parameters/                     # (legacy) CloudFormation parameters
├── scripts/
│   └── deploy-customer.sh          # (legacy) CloudFormation deployment script
└── README.md
```

## Quick Start

1. Copy and configure your variables:
   ```bash
   cp terraform/terraform.tfvars.example terraform/terraform.tfvars
   # Edit terraform.tfvars with your values
   ```

2. Deploy:
   ```bash
   ./terraform/scripts/deploy.sh
   ```

For detailed configuration options, operations, scaling, and troubleshooting, see the [full deployment guide](https://docs.getomni.co/deployment/aws-terraform).

## Modules

| Module | Description |
|--------|-------------|
| **networking** | VPC, public/private subnets, NAT gateway, security groups |
| **database** | RDS PostgreSQL instance with ParadeDB extensions |
| **compute** | ECS Fargate cluster, task definitions, service discovery |
| **cache** | ElastiCache Redis for sessions and caching |
| **storage** | S3 buckets for artifacts and data |
| **loadbalancer** | Application Load Balancer with TLS termination |
| **secrets** | AWS Secrets Manager for credentials |
| **monitoring** | CloudWatch log groups and alarms |
| **migrations** | Lambda function to run database migrations |

## AWS CloudFormation

The `scripts/deploy-customer.sh` script and `parameters/` directory are from an earlier CloudFormation-based deployment approach and are deprecated. Use the Terraform setup above instead.
