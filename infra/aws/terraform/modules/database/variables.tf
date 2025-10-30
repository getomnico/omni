variable "customer_name" {
  description = "Customer name for resource naming"
  type        = string
}

variable "environment" {
  description = "Environment (production, staging, development)"
  type        = string
  default     = "production"
}

variable "use_rds" {
  description = "Use AWS RDS PostgreSQL instead of self-hosted ParadeDB"
  type        = bool
  default     = false
}

# ============================================================================
# RDS-specific variables
# ============================================================================

variable "instance_class" {
  description = "RDS instance type"
  type        = string
  default     = "db.t3.micro"
}

variable "allocated_storage" {
  description = "Allocated storage in GB"
  type        = number
  default     = 20
}

variable "backup_retention_period" {
  description = "Backup retention period in days"
  type        = number
  default     = 7
}

variable "multi_az" {
  description = "Enable Multi-AZ deployment"
  type        = bool
  default     = false
}

variable "deletion_protection" {
  description = "Enable deletion protection"
  type        = bool
  default     = false
}

variable "skip_final_snapshot" {
  description = "Skip final DB snapshot on deletion"
  type        = bool
  default     = false
}

variable "security_group_id" {
  description = "Security group ID for RDS database"
  type        = string
  default     = ""
}

# ============================================================================
# ParadeDB-specific variables
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
  default     = "paradedb/paradedb:0.18.15-pg17"
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
# Common variables (used by both RDS and ParadeDB)
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

variable "database_password" {
  description = "PostgreSQL master password (only used for RDS)"
  type        = string
  sensitive   = true
  default     = ""
}

variable "subnet_ids" {
  description = "List of subnet IDs for database"
  type        = list(string)
}
