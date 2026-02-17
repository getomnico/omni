output "internal_ip" {
  description = "ParadeDB internal IP address"
  value       = google_compute_address.paradedb.address
}

output "port" {
  description = "Database port"
  value       = 5432
}

output "database_name" {
  description = "Database name"
  value       = var.database_name
}
