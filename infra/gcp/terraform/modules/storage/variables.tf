variable "customer_name" {
  description = "Customer name for resource naming"
  type        = string
}

variable "region" {
  description = "GCP region for bucket location"
  type        = string
}

variable "cloud_run_sa_email" {
  description = "Cloud Run service account email for HMAC key generation"
  type        = string
}
