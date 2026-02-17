resource "google_storage_bucket" "content" {
  name     = "omni-${var.customer_name}-content"
  location = var.region

  uniform_bucket_level_access = true
  public_access_prevention    = "enforced"

  versioning {
    enabled = true
  }

  force_destroy = false
}

resource "google_storage_bucket" "batch" {
  name     = "omni-${var.customer_name}-batch-inference"
  location = var.region

  uniform_bucket_level_access = true
  public_access_prevention    = "enforced"

  lifecycle_rule {
    condition {
      age = 7
    }
    action {
      type = "Delete"
    }
  }

  force_destroy = true
}

# HMAC key for S3-compatible access to GCS
resource "google_storage_hmac_key" "cloud_run" {
  service_account_email = var.cloud_run_sa_email
}
