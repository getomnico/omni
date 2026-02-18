locals {
  common_tags = {
    Application = "Omni"
    Customer    = var.customer_name
    Environment = var.environment
    ManagedBy   = "Terraform"
  }
}

resource "random_password" "database" {
  length  = 48
  special = false
  upper   = true
  lower   = true
  numeric = true
}

resource "aws_secretsmanager_secret" "database_password" {
  name        = "omni/${var.customer_name}/database"
  description = "Database password for Omni (alphanumeric only)"

  tags = merge(local.common_tags, {
    Name = "omni-${var.customer_name}-db-password"
  })
}

resource "aws_secretsmanager_secret_version" "database_password" {
  secret_id = aws_secretsmanager_secret.database_password.id
  secret_string = jsonencode({
    username = var.database_username
    password = random_password.database.result
  })
}

resource "aws_secretsmanager_secret" "jina_api_key" {
  name        = "omni/${var.customer_name}/jina-api-key"
  description = "JINA AI API key for embedding generation"

  tags = merge(local.common_tags, {
    Name = "omni-${var.customer_name}-jina-api-key"
  })
}

resource "aws_secretsmanager_secret_version" "jina_api_key" {
  secret_id     = aws_secretsmanager_secret.jina_api_key.id
  secret_string = var.jina_api_key
}

resource "random_password" "encryption_key" {
  length           = 64
  special          = true
  upper            = true
  lower            = true
  numeric          = true
  override_special = "!@#$%^&*()-_=+[]{}|;:,.<>?"
}

resource "aws_secretsmanager_secret" "encryption_key" {
  name        = "omni/${var.customer_name}/encryption-key"
  description = "Encryption key for sensitive data"

  tags = merge(local.common_tags, {
    Name = "omni-${var.customer_name}-encryption-key"
  })
}

resource "aws_secretsmanager_secret_version" "encryption_key" {
  secret_id = aws_secretsmanager_secret.encryption_key.id
  secret_string = jsonencode({
    key = random_password.encryption_key.result
  })
}

resource "random_password" "encryption_salt" {
  length           = 32
  special          = true
  upper            = true
  lower            = true
  numeric          = true
  override_special = "!@#$%^&*()-_=+[]{}|;:,.<>?"
}

resource "aws_secretsmanager_secret" "encryption_salt" {
  name        = "omni/${var.customer_name}/encryption-salt"
  description = "Encryption salt for sensitive data"

  tags = merge(local.common_tags, {
    Name = "omni-${var.customer_name}-encryption-salt"
  })
}

resource "aws_secretsmanager_secret_version" "encryption_salt" {
  secret_id = aws_secretsmanager_secret.encryption_salt.id
  secret_string = jsonencode({
    salt = random_password.encryption_salt.result
  })
}

