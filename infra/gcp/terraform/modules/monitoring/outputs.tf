output "log_bucket_id" {
  description = "Cloud Logging bucket ID"
  value       = google_logging_project_bucket_config.omni.bucket_id
}

output "log_sink_name" {
  description = "Cloud Logging sink name"
  value       = google_logging_project_sink.omni.name
}
