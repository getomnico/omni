variable "customer_name" {
  description = "Customer name for resource naming"
  type        = string
}

variable "environment" {
  description = "Environment (production, staging, development)"
  type        = string
  default     = "production"
}

variable "project_id" {
  description = "GCP project ID"
  type        = string
}

variable "region" {
  description = "GCP region"
  type        = string
}

variable "github_org" {
  description = "GitHub organization for container images"
  type        = string
}

variable "custom_domain" {
  description = "Custom domain for the application"
  type        = string
}

variable "vpc_connector_id" {
  description = "VPC Access Connector ID for Cloud Run"
  type        = string
}

variable "cloud_run_cpu" {
  description = "Cloud Run CPU allocation"
  type        = string
  default     = "1"
}

variable "cloud_run_memory" {
  description = "Cloud Run memory allocation"
  type        = string
  default     = "1Gi"
}

variable "cloud_run_min_instances" {
  description = "Minimum Cloud Run instances"
  type        = number
  default     = 0
}

variable "cloud_run_max_instances" {
  description = "Maximum Cloud Run instances"
  type        = number
  default     = 2
}

# Database
variable "database_host" {
  description = "ParadeDB internal IP"
  type        = string
}

variable "database_port" {
  description = "Database port"
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

# Redis
variable "redis_url" {
  description = "Redis URL (redis://host:port)"
  type        = string
}

# Secrets (Secret Manager secret IDs)
variable "database_password_secret_id" {
  description = "Secret Manager secret ID for database password"
  type        = string
}

variable "jina_api_key_secret_id" {
  description = "Secret Manager secret ID for JINA API key"
  type        = string
}

variable "encryption_key_secret_id" {
  description = "Secret Manager secret ID for encryption key"
  type        = string
}

variable "encryption_salt_secret_id" {
  description = "Secret Manager secret ID for encryption salt"
  type        = string
}

variable "all_secret_ids" {
  description = "List of all secret IDs for IAM binding"
  type        = list(string)
}

variable "resend_api_key" {
  description = "Resend API key for emails"
  type        = string
  default     = "CONFIGURE_RESEND_API_KEY"
  sensitive   = true
}

# Storage
variable "content_bucket_name" {
  description = "GCS bucket name for content storage"
  type        = string
}

variable "batch_bucket_name" {
  description = "GCS bucket name for batch inference"
  type        = string
}

variable "hmac_access_key" {
  description = "HMAC access key for S3-compatible GCS access"
  type        = string
}

variable "hmac_secret" {
  description = "HMAC secret for S3-compatible GCS access"
  type        = string
  sensitive   = true
}

variable "embedding_api_url" {
  description = "Embedding API base URL"
  type        = string
}

variable "cloud_run_sa_email" {
  description = "Cloud Run service account email (created at root level)"
  type        = string
}
