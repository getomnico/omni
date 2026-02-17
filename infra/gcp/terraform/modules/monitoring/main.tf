# Cloud Logging is automatic for Cloud Run and GCE.
# This module sets up a dedicated log bucket with custom retention.

resource "google_logging_project_bucket_config" "omni" {
  project        = var.project_id
  location       = "global"
  bucket_id      = "omni-${var.customer_name}-logs"
  retention_days = var.log_retention_days
}

resource "google_logging_project_sink" "omni" {
  name        = "omni-${var.customer_name}-sink"
  destination = "logging.googleapis.com/projects/${var.project_id}/locations/global/buckets/${google_logging_project_bucket_config.omni.bucket_id}"

  filter = "resource.type = \"cloud_run_revision\" OR resource.type = \"gce_instance\" OR resource.type = \"cloud_run_job\""

  unique_writer_identity = true
}
