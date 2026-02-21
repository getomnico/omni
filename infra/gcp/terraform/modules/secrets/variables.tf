variable "customer_name" {
  description = "Customer name for resource naming"
  type        = string
}

variable "environment" {
  description = "Environment (production, staging, development)"
  type        = string
  default     = "production"
}

variable "database_username" {
  description = "PostgreSQL master username"
  type        = string
  default     = "omni"
}


