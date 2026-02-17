resource "google_redis_instance" "main" {
  name               = "omni-${var.customer_name}-redis"
  tier               = var.tier
  memory_size_gb     = var.memory_size_gb
  region             = var.region
  redis_version      = "REDIS_7_0"
  authorized_network = var.network_id

  depends_on = [var.private_service_connection]
}
