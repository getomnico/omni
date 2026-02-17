variable "customer_name" {
  description = "Customer name for resource naming"
  type        = string
}

variable "region" {
  description = "GCP region for the serverless NEG"
  type        = string
}

variable "custom_domain" {
  description = "Custom domain for the managed SSL certificate"
  type        = string
}

variable "web_service_name" {
  description = "Cloud Run web service name for the serverless NEG"
  type        = string
}
