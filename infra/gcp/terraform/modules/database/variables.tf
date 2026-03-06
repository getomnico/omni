variable "customer_name" {
  description = "Customer name for resource naming"
  type        = string
}

variable "environment" {
  description = "Environment (production, staging, development)"
  type        = string
  default     = "production"
}

variable "region" {
  description = "GCP region"
  type        = string
}

variable "zone" {
  description = "GCP zone for the VM"
  type        = string
  default     = ""
}

variable "machine_type" {
  description = "GCE machine type for ParadeDB"
  type        = string
  default     = "e2-small"
}

variable "disk_size_gb" {
  description = "Persistent disk size in GB for PostgreSQL data"
  type        = number
  default     = 50
}

variable "container_image" {
  description = "Docker image for ParadeDB"
  type        = string
  default     = "paradedb/paradedb:0.20.6-pg17"
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

variable "database_password" {
  description = "PostgreSQL password"
  type        = string
  sensitive   = true
}

variable "network_id" {
  description = "VPC network ID"
  type        = string
}

variable "subnet_id" {
  description = "Subnet ID for the VM"
  type        = string
}

variable "project_id" {
  description = "GCP project ID"
  type        = string
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
