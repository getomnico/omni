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


variable "encryption_key_arn" {
  description = "ARN of encryption key secret"
  type        = string
}

variable "encryption_salt_arn" {
  description = "ARN of encryption salt secret"
  type        = string
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
