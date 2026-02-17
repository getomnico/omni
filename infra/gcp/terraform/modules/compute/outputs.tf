output "web_service_uri" {
  description = "Cloud Run web service URI"
  value       = local.service_url["web"]
}

output "web_service_name" {
  description = "Cloud Run web service name (for LB NEG)"
  value       = google_cloud_run_v2_service.web.name
}

output "searcher_service_uri" {
  description = "Cloud Run searcher service URI"
  value       = local.service_url["searcher"]
}

output "indexer_service_uri" {
  description = "Cloud Run indexer service URI"
  value       = local.service_url["indexer"]
}

output "ai_service_uri" {
  description = "Cloud Run AI service URI"
  value       = local.service_url["ai"]
}

output "connector_manager_service_uri" {
  description = "Cloud Run connector manager service URI"
  value       = local.service_url["connector-mgr"]
}

output "connector_service_uris" {
  description = "Map of connector name to Cloud Run URI"
  value       = { for k, _ in local.connectors : k => local.service_url["${k}-conn"] }
}

output "cloud_run_sa_email" {
  description = "Cloud Run service account email"
  value       = var.cloud_run_sa_email
}
