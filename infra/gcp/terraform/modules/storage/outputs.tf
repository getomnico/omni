output "content_bucket_name" {
  description = "GCS bucket name for content storage"
  value       = google_storage_bucket.content.name
}

output "batch_bucket_name" {
  description = "GCS bucket name for batch inference"
  value       = google_storage_bucket.batch.name
}

output "hmac_access_key" {
  description = "HMAC access key ID for S3-compatible access"
  value       = google_storage_hmac_key.cloud_run.access_id
}

output "hmac_secret" {
  description = "HMAC secret for S3-compatible access"
  value       = google_storage_hmac_key.cloud_run.secret
  sensitive   = true
}
