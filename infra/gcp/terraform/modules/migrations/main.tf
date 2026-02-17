resource "google_cloud_run_v2_job" "migrator" {
  name     = "omni-${var.customer_name}-migrator"
  location = var.region

  template {
    template {
      service_account = var.cloud_run_sa_email

      vpc_access {
        connector = var.vpc_connector_id
        egress    = "PRIVATE_RANGES_ONLY"
      }

      containers {
        image = "ghcr.io/${var.github_org}/omni/omni-migrator:latest"

        resources {
          limits = {
            cpu    = "1"
            memory = "512Mi"
          }
        }

        dynamic "env" {
          for_each = {
            DATABASE_HOST     = var.database_host
            DATABASE_PORT     = tostring(var.database_port)
            DATABASE_NAME     = var.database_name
            DATABASE_USERNAME = var.database_username
            DATABASE_SSL      = "false"
            REDIS_URL         = var.redis_url
          }
          content {
            name  = env.key
            value = env.value
          }
        }

        env {
          name = "DATABASE_PASSWORD"
          value_source {
            secret_key_ref {
              secret  = var.database_password_secret_id
              version = "latest"
            }
          }
        }
      }

      max_retries = 1
      timeout     = "300s"
    }
  }
}

resource "null_resource" "run_migrations" {
  triggers = {
    job_id = google_cloud_run_v2_job.migrator.id
  }

  provisioner "local-exec" {
    command = <<-EOT
      gcloud run jobs execute ${google_cloud_run_v2_job.migrator.name} \
        --region=${var.region} \
        --project=${var.project_id} \
        --wait
    EOT
  }
}
