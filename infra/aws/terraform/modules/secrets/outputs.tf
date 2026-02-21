output "database_password_arn" {
  description = "ARN of database password secret"
  value       = aws_secretsmanager_secret.database_password.arn
}

output "database_password" {
  description = "Database password value"
  value       = random_password.database.result
  sensitive   = true
}


output "encryption_key_arn" {
  description = "ARN of encryption key secret"
  value       = aws_secretsmanager_secret.encryption_key.arn
}

output "encryption_salt_arn" {
  description = "ARN of encryption salt secret"
  value       = aws_secretsmanager_secret.encryption_salt.arn
}
