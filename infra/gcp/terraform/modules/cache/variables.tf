variable "customer_name" {
  description = "Customer name for resource naming"
  type        = string
}

variable "region" {
  description = "GCP region"
  type        = string
}

variable "tier" {
  description = "Memorystore Redis tier (BASIC or STANDARD_HA)"
  type        = string
  default     = "BASIC"
}

variable "memory_size_gb" {
  description = "Redis memory size in GB"
  type        = number
  default     = 1
}

variable "network_id" {
  description = "VPC network ID for Redis private connection"
  type        = string
}

variable "private_service_connection" {
  description = "Private service connection ID (for depends_on)"
  type        = string
}
