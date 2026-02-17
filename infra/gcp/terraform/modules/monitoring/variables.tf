variable "customer_name" {
  description = "Customer name for resource naming"
  type        = string
}

variable "project_id" {
  description = "GCP project ID"
  type        = string
}

variable "log_retention_days" {
  description = "Cloud Logging retention in days"
  type        = number
  default     = 30
}
