# Required Variables
variable "project_id" {
  description = "GCP project ID"
  type        = string
}

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
  description = "GCP region"
  type        = string
  default     = "us-central1"
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

variable "paradedb_machine_type" {
  description = "GCE machine type for ParadeDB"
  type        = string
  default     = "e2-small"
}

variable "paradedb_disk_size_gb" {
  description = "Persistent disk size in GB for ParadeDB data"
  type        = number
  default     = 50
}

variable "paradedb_container_image" {
  description = "Docker image for ParadeDB"
  type        = string
  default     = "paradedb/paradedb:0.20.6-pg17"
}

variable "redis_tier" {
  description = "Memorystore Redis tier (BASIC or STANDARD_HA)"
  type        = string
  default     = "BASIC"

  validation {
    condition     = contains(["BASIC", "STANDARD_HA"], var.redis_tier)
    error_message = "Redis tier must be BASIC or STANDARD_HA."
  }
}

variable "redis_memory_size_gb" {
  description = "Memorystore Redis memory size in GB"
  type        = number
  default     = 1
}

variable "cloud_run_cpu" {
  description = "Cloud Run CPU allocation (e.g., 1, 2, 4)"
  type        = string
  default     = "1"
}

variable "cloud_run_memory" {
  description = "Cloud Run memory allocation (e.g., 512Mi, 1Gi, 2Gi)"
  type        = string
  default     = "1Gi"
}

variable "cloud_run_min_instances" {
  description = "Minimum number of Cloud Run instances per service"
  type        = number
  default     = 0
}

variable "cloud_run_max_instances" {
  description = "Maximum number of Cloud Run instances per service"
  type        = number
  default     = 2
}

variable "custom_domain" {
  description = "Custom domain for the application (e.g., demo.getomni.co)"
  type        = string
}

variable "log_retention_days" {
  description = "Cloud Logging retention in days"
  type        = number
  default     = 30
}

variable "resend_api_key" {
  description = "Resend API key for emails"
  type        = string
  default     = "CONFIGURE_RESEND_API_KEY"
  sensitive   = true
}

variable "embedding_api_url" {
  description = "Embedding API base URL"
  type        = string
  default     = "https://api.jina.ai/v1"
}

variable "vpc_cidr" {
  description = "CIDR block for VPC subnet"
  type        = string
  default     = "10.0.0.0/16"
}
