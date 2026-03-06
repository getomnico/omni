variable "customer_name" {
  description = "Customer name for resource naming"
  type        = string
}

variable "environment" {
  description = "Environment (production, staging, development)"
  type        = string
  default     = "production"
}

# ============================================================================
# ParadeDB Configuration
# ============================================================================

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
  default     = "paradedb/paradedb:0.20.6-pg17"
}

variable "vpc_id" {
  description = "VPC ID for ParadeDB security group"
  type        = string
  default     = ""
}

variable "ecs_security_group_id" {
  description = "ECS security group ID to allow connections to ParadeDB"
  type        = string
  default     = ""
}

variable "ecs_cluster_name" {
  description = "ECS cluster name for ParadeDB service"
  type        = string
  default     = ""
}

variable "service_discovery_namespace_id" {
  description = "Service discovery namespace ID for ParadeDB"
  type        = string
  default     = ""
}

variable "database_password_secret_arn" {
  description = "ARN of the database password secret in Secrets Manager"
  type        = string
  default     = ""
}

variable "region" {
  description = "AWS region (passed from root module)"
  type        = string
}

# ============================================================================
# Database Configuration
# ============================================================================

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

variable "subnet_ids" {
  description = "List of subnet IDs for database"
  type        = list(string)
}

# ============================================================================
# PostgreSQL Tuning
# ============================================================================

variable "pg_shared_buffers" {
  description = "PostgreSQL shared_buffers setting"
  type        = string
  default     = "1GB"
}

variable "pg_max_parallel_workers_per_gather" {
  description = "PostgreSQL max_parallel_workers_per_gather (should match BM25 target_segment_count)"
  type        = number
  default     = 2
}

variable "pg_max_parallel_workers" {
  description = "PostgreSQL max_parallel_workers (must be >= max_parallel_maintenance_workers)"
  type        = number
  default     = 4
}

variable "pg_max_parallel_maintenance_workers" {
  description = "PostgreSQL max_parallel_maintenance_workers (for REINDEX/CREATE INDEX)"
  type        = number
  default     = 2
}

variable "pg_max_worker_processes" {
  description = "PostgreSQL max_worker_processes"
  type        = number
  default     = 8
}
