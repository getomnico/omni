# Required Variables
variable "customer_name" {
  description = "Customer name for resource naming (e.g., acme-corp)"
  type        = string

  validation {
    condition     = can(regex("^[a-z0-9-]+$", var.customer_name))
    error_message = "Customer name must contain only lowercase letters, numbers, and hyphens."
  }
}

variable "github_org" {
  description = "GitHub organization for container images (e.g., omni-platform)"
  type        = string
}

variable "jina_api_key" {
  description = "JINA AI API key for embedding generation"
  type        = string
  sensitive   = true
}

# Optional Variables with Defaults
variable "region" {
  description = "AWS region"
  type        = string
  default     = "us-east-1"
}

variable "environment" {
  description = "Environment (production, staging, development)"
  type        = string
  default     = "production"

  validation {
    condition     = contains(["production", "staging", "development"], var.environment)
    error_message = "Environment must be one of: production, staging, development."
  }
}

# Database Configuration
variable "use_rds" {
  description = "Use AWS RDS PostgreSQL instead of self-hosted ParadeDB"
  type        = bool
  default     = false
}

variable "database_name" {
  description = "PostgreSQL database name"
  type        = string
  default     = "omni"
}

variable "database_username" {
  description = "PostgreSQL master username"
  type        = string
  default     = "omni"
}

# RDS-specific variables
variable "db_instance_class" {
  description = "RDS instance type"
  type        = string
  default     = "db.t3.micro"
}

variable "db_allocated_storage" {
  description = "Allocated storage in GB for RDS"
  type        = number
  default     = 20
}

variable "db_backup_retention_period" {
  description = "Backup retention period in days"
  type        = number
  default     = 7
}

variable "db_multi_az" {
  description = "Enable Multi-AZ deployment for RDS"
  type        = bool
  default     = false
}

variable "db_deletion_protection" {
  description = "Enable deletion protection for RDS"
  type        = bool
  default     = false
}

variable "skip_final_snapshot" {
  description = "Skip final DB snapshot on deletion (true for dev, false for production)"
  type        = bool
  default     = false
}

# ParadeDB-specific variables
variable "paradedb_instance_type" {
  description = "EC2 instance type for ParadeDB"
  type        = string
  default     = "t3.small"
}

variable "paradedb_volume_size" {
  description = "EBS volume size in GB for ParadeDB data"
  type        = number
  default     = 50
}

variable "paradedb_container_image" {
  description = "Docker image for ParadeDB"
  type        = string
  default     = "paradedb/paradedb:0.18.15-pg17"
}

# Cache Configuration
variable "redis_node_type" {
  description = "ElastiCache Redis node type"
  type        = string
  default     = "cache.t3.micro"
}

variable "redis_engine_version" {
  description = "Redis engine version"
  type        = string
  default     = "7.1"
}

# ECS Configuration
variable "ecs_task_cpu" {
  description = "ECS task CPU units (256, 512, 1024, 2048, 4096)"
  type        = string
  default     = "512"

  validation {
    condition     = contains(["256", "512", "1024", "2048", "4096"], var.ecs_task_cpu)
    error_message = "ECS task CPU must be one of: 256, 512, 1024, 2048, 4096."
  }
}

variable "ecs_task_memory" {
  description = "ECS task memory in MB"
  type        = string
  default     = "1024"
}

variable "ecs_desired_count" {
  description = "Desired number of ECS tasks per service"
  type        = number
  default     = 1
}

# Load Balancer Configuration
variable "ssl_certificate_arn" {
  description = "ARN of ACM certificate for HTTPS (leave empty for HTTP-only)"
  type        = string
  default     = ""
}

variable "custom_domain" {
  description = "Custom domain for the application (e.g., demo.getomni.co)"
  type        = string
}

variable "alb_deletion_protection" {
  description = "Enable deletion protection for ALB"
  type        = bool
  default     = false
}

# Monitoring Configuration
variable "log_retention_days" {
  description = "CloudWatch Logs retention in days"
  type        = number
  default     = 30
}

# Application Configuration
variable "google_client_id" {
  description = "Google OAuth client ID"
  type        = string
  default     = "CONFIGURE_GOOGLE_CLIENT_ID"
}

variable "google_client_secret" {
  description = "Google OAuth client secret"
  type        = string
  default     = "CONFIGURE_GOOGLE_CLIENT_SECRET"
  sensitive   = true
}

variable "resend_api_key" {
  description = "Resend API key for emails"
  type        = string
  default     = "CONFIGURE_RESEND_API_KEY"
  sensitive   = true
}

# Networking Configuration
variable "vpc_cidr" {
  description = "CIDR block for VPC"
  type        = string
  default     = "10.0.0.0/16"
}
