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

variable "vpc_cidr" {
  description = "CIDR block for VPC subnet"
  type        = string
  default     = "10.0.0.0/16"
}

# AI Service Configuration
variable "embedding_model" {
  description = "Embedding model name"
  type        = string
  default     = "jina-embeddings-v3"
}

variable "embedding_max_model_len" {
  description = "Maximum model input length for embeddings"
  type        = string
  default     = "8192"
}

variable "ai_workers" {
  description = "Number of AI service workers"
  type        = string
  default     = "1"
}

# Batch Embedding Configuration
variable "embedding_batch_min_documents" {
  description = "Minimum documents before triggering batch embedding"
  type        = string
  default     = "100"
}

variable "embedding_batch_max_documents" {
  description = "Maximum documents per batch embedding job"
  type        = string
  default     = "50000"
}

variable "embedding_batch_accumulation_timeout_seconds" {
  description = "Timeout in seconds for batch embedding accumulation"
  type        = string
  default     = "300"
}

variable "embedding_batch_accumulation_poll_interval" {
  description = "Poll interval in seconds for batch embedding accumulation"
  type        = string
  default     = "10"
}

variable "embedding_batch_monitor_poll_interval" {
  description = "Poll interval in seconds for batch embedding job monitoring"
  type        = string
  default     = "30"
}

# Searcher Configuration
variable "semantic_search_timeout_ms" {
  description = "Timeout in milliseconds for semantic search"
  type        = string
  default     = "1000"
}

variable "rag_context_window" {
  description = "Number of context chunks for RAG"
  type        = string
  default     = "2"
}

# Connector Manager Configuration
variable "max_concurrent_syncs" {
  description = "Maximum concurrent connector syncs"
  type        = string
  default     = "10"
}

variable "max_concurrent_syncs_per_type" {
  description = "Maximum concurrent syncs per connector type"
  type        = string
  default     = "3"
}

variable "scheduler_poll_interval_seconds" {
  description = "Scheduler poll interval in seconds"
  type        = string
  default     = "60"
}

variable "stale_sync_timeout_minutes" {
  description = "Timeout in minutes for stale syncs"
  type        = string
  default     = "10"
}

# Google Connector Configuration
variable "google_webhook_url" {
  description = "Google webhook URL for push notifications"
  type        = string
  default     = ""
}

variable "google_sync_interval_seconds" {
  description = "Google connector sync interval in seconds"
  type        = string
  default     = ""
}

variable "google_max_age_days" {
  description = "Maximum age in days for Google documents"
  type        = string
  default     = "730"
}

variable "webhook_renewal_check_interval_seconds" {
  description = "Interval in seconds for webhook renewal checks"
  type        = string
  default     = "3600"
}

# Common Service Configuration
variable "rust_log" {
  description = "Rust log level"
  type        = string
  default     = "info"
}

variable "db_max_connections" {
  description = "Maximum database connections per service"
  type        = string
  default     = "10"
}

variable "db_acquire_timeout_seconds" {
  description = "Database connection acquire timeout in seconds"
  type        = string
  default     = "3"
}

variable "session_cookie_name" {
  description = "Session cookie name"
  type        = string
  default     = "auth-session"
}

variable "session_duration_days" {
  description = "Session duration in days"
  type        = string
  default     = "7"
}

variable "ai_answer_enabled" {
  description = "Enable AI answer feature"
  type        = string
  default     = "true"
}
