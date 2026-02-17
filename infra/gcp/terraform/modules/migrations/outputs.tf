output "job_name" {
  description = "Cloud Run Job name for the migrator"
  value       = google_cloud_run_v2_job.migrator.name
}
