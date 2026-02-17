output "host" {
  description = "Memorystore Redis host"
  value       = google_redis_instance.main.host
}

output "port" {
  description = "Memorystore Redis port"
  value       = google_redis_instance.main.port
}

output "redis_url" {
  description = "Full Redis URL"
  value       = "redis://${google_redis_instance.main.host}:${google_redis_instance.main.port}"
}
