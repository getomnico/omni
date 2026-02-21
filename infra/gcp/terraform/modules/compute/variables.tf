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

variable "cloud_run_sa_email" {
  description = "Cloud Run service account email (created at root level)"
  type        = string
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
