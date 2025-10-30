variable "customer_name" {
  description = "Customer name for resource naming"
  type        = string
}

variable "environment" {
  description = "Environment (production, staging, development)"
  type        = string
  default     = "production"
}

variable "github_org" {
  description = "GitHub organization for container images"
  type        = string
}

variable "vpc_id" {
  description = "VPC ID"
  type        = string
}

variable "subnet_ids" {
  description = "List of private subnet IDs for ECS services"
  type        = list(string)
}

variable "security_group_id" {
  description = "Security group ID for ECS services"
  type        = string
}

variable "cluster_name" {
  description = "ECS cluster name (passed from root)"
  type        = string
}

variable "cluster_arn" {
  description = "ECS cluster ARN (passed from root)"
  type        = string
}

variable "service_discovery_namespace_id" {
  description = "Service discovery namespace ID (passed from root)"
  type        = string
}

variable "alb_target_group_arn" {
  description = "ALB target group ARN for web service"
  type        = string
}

variable "alb_dns_name" {
  description = "ALB DNS name"
  type        = string
}

variable "custom_domain" {
  description = "Custom domain for the application"
  type        = string
}

variable "task_cpu" {
  description = "ECS task CPU units"
  type        = string
  default     = "512"
}

variable "task_memory" {
  description = "ECS task memory (MB)"
  type        = string
  default     = "1024"
}

variable "desired_count" {
  description = "Desired number of tasks per service"
  type        = number
  default     = 1
}

variable "database_endpoint" {
  description = "RDS database endpoint"
  type        = string
}

variable "database_port" {
  description = "RDS database port"
  type        = number
}

variable "database_name" {
  description = "Database name"
  type        = string
}

variable "database_username" {
  description = "Database username"
  type        = string
}

variable "redis_endpoint" {
  description = "Redis cluster endpoint"
  type        = string
}

variable "redis_port" {
  description = "Redis port"
  type        = number
}

variable "log_group_name" {
  description = "CloudWatch Log Group name"
  type        = string
}

variable "region" {
  description = "AWS region"
  type        = string
}

variable "database_password_arn" {
  description = "ARN of database password secret"
  type        = string
}

variable "jina_api_key_arn" {
  description = "ARN of JINA API key secret"
  type        = string
}

variable "encryption_key_arn" {
  description = "ARN of encryption key secret"
  type        = string
}

variable "encryption_salt_arn" {
  description = "ARN of encryption salt secret"
  type        = string
}

variable "session_secret_arn" {
  description = "ARN of session secret"
  type        = string
}

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

variable "otel_endpoint" {
  description = "OpenTelemetry collector endpoint (leave empty to disable)"
  type        = string
  default     = ""
}

variable "service_version" {
  description = "Service version for OpenTelemetry"
  type        = string
  default     = "0.1.0"
}

# Storage variables for S3 access
variable "content_bucket_arn" {
  description = "ARN of the S3 bucket for content storage"
  type        = string
}

variable "content_bucket_name" {
  description = "Name of the S3 bucket for content storage"
  type        = string
}

variable "batch_bucket_arn" {
  description = "ARN of the S3 bucket for batch inference"
  type        = string
}

variable "batch_bucket_name" {
  description = "Name of the S3 bucket for batch inference"
  type        = string
}

variable "bedrock_batch_role_arn" {
  description = "ARN of the IAM role for Bedrock batch inference"
  type        = string
}
