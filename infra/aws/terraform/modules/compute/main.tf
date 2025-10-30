locals {
  common_tags = {
    Application = "Omni"
    Customer    = var.customer_name
    Environment = var.environment
    ManagedBy   = "Terraform"
  }

  app_url = "https://${var.custom_domain}"
}

data "aws_caller_identity" "current" {}

# Note: ECS cluster and service discovery namespace are now created in the root module
# and passed as variables to avoid circular dependency with the database module
