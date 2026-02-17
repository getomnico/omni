output "omni_url" {
  description = "URL to access Omni application"
  value       = "https://${var.custom_domain}"
}

output "load_balancer_ip" {
  description = "Global HTTPS Load Balancer external IP"
  value       = module.loadbalancer.external_ip
}

output "database_internal_ip" {
  description = "ParadeDB GCE instance internal IP"
  value       = module.database.internal_ip
}

output "redis_host" {
  description = "Memorystore Redis host"
  value       = module.cache.host
}

output "web_service_uri" {
  description = "Cloud Run web service URI"
  value       = module.compute.web_service_uri
}

output "project_id" {
  description = "GCP project ID"
  value       = var.project_id
}

output "region" {
  description = "GCP region"
  value       = var.region
}

output "content_bucket_name" {
  description = "GCS bucket name for content storage"
  value       = module.storage.content_bucket_name
}

output "batch_bucket_name" {
  description = "GCS bucket name for batch inference"
  value       = module.storage.batch_bucket_name
}

output "next_steps" {
  description = "What to do next"
  value       = <<-EOT
    Omni has been deployed successfully!

    Access your application:
       https://${var.custom_domain}

    DNS Configuration:
       Point ${var.custom_domain} A record to ${module.loadbalancer.external_ip}

    Monitor your deployment:
       Cloud Console: https://console.cloud.google.com/run?project=${var.project_id}
       Logs: https://console.cloud.google.com/logs?project=${var.project_id}

    Configure integrations:
       Update these variables in terraform.tfvars:
       - resend_api_key

    View logs:
       gcloud logging read 'resource.type="cloud_run_revision"' --project=${var.project_id} --limit=50
  EOT
}
