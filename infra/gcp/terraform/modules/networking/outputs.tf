output "network_id" {
  description = "VPC network ID"
  value       = google_compute_network.main.id
}

output "network_name" {
  description = "VPC network name"
  value       = google_compute_network.main.name
}

output "private_subnet_id" {
  description = "Private subnet ID"
  value       = google_compute_subnetwork.private.id
}

output "private_subnet_name" {
  description = "Private subnet name"
  value       = google_compute_subnetwork.private.name
}

output "vpc_connector_id" {
  description = "VPC Access Connector ID for Cloud Run"
  value       = google_vpc_access_connector.main.id
}

output "private_service_connection" {
  description = "Private service networking connection (for depends_on)"
  value       = google_service_networking_connection.private_service.id
}
