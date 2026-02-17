output "external_ip" {
  description = "External IP address of the load balancer"
  value       = google_compute_global_address.lb.address
}

output "lb_url" {
  description = "HTTPS URL via the load balancer"
  value       = "https://${var.custom_domain}"
}
