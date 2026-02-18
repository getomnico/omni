output "database_password_secret_id" {
  description = "Secret Manager secret ID for database password"
  value       = google_secret_manager_secret.database_password.secret_id
}

output "database_password" {
  description = "Database password value"
  value       = random_password.database.result
  sensitive   = true
}

output "jina_api_key_secret_id" {
  description = "Secret Manager secret ID for JINA API key"
  value       = google_secret_manager_secret.jina_api_key.secret_id
}

output "encryption_key_secret_id" {
  description = "Secret Manager secret ID for encryption key"
  value       = google_secret_manager_secret.encryption_key.secret_id
}

output "encryption_salt_secret_id" {
  description = "Secret Manager secret ID for encryption salt"
  value       = google_secret_manager_secret.encryption_salt.secret_id
}

output "all_secret_ids" {
  description = "List of all secret IDs for IAM binding"
  value = [
    google_secret_manager_secret.database_password.secret_id,
    google_secret_manager_secret.jina_api_key.secret_id,
    google_secret_manager_secret.encryption_key.secret_id,
    google_secret_manager_secret.encryption_salt.secret_id,
  ]
}
