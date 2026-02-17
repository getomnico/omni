variable "customer_name" {
  description = "Customer name for resource naming"
  type        = string
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

variable "vpc_connector_id" {
  description = "VPC Access Connector ID"
  type        = string
}

variable "cloud_run_sa_email" {
  description = "Cloud Run service account email"
  type        = string
}

# Database connection
variable "database_host" {
  description = "Database host"
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

variable "database_password_secret_id" {
  description = "Secret Manager secret ID for database password"
  type        = string
}

variable "redis_url" {
  description = "Redis URL"
  type        = string
}
