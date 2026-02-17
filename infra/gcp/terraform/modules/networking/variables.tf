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

variable "vpc_cidr" {
  description = "CIDR block for private subnet"
  type        = string
  default     = "10.0.0.0/16"
}
