# Static external IP
resource "google_compute_global_address" "lb" {
  name = "omni-${var.customer_name}-lb-ip"
}

# Google-managed SSL certificate
resource "google_compute_managed_ssl_certificate" "main" {
  name = "omni-${var.customer_name}-cert"

  managed {
    domains = [var.custom_domain]
  }
}

# Serverless NEG pointing to the Cloud Run web service
resource "google_compute_region_network_endpoint_group" "web" {
  name                  = "omni-${var.customer_name}-web-neg"
  network_endpoint_type = "SERVERLESS"
  region                = var.region

  cloud_run {
    service = var.web_service_name
  }
}

# Backend service
resource "google_compute_backend_service" "web" {
  name                  = "omni-${var.customer_name}-web-backend"
  protocol              = "HTTP"
  load_balancing_scheme = "EXTERNAL_MANAGED"

  backend {
    group = google_compute_region_network_endpoint_group.web.id
  }
}

# URL map
resource "google_compute_url_map" "main" {
  name            = "omni-${var.customer_name}-url-map"
  default_service = google_compute_backend_service.web.id
}

# HTTP-to-HTTPS redirect URL map
resource "google_compute_url_map" "http_redirect" {
  name = "omni-${var.customer_name}-http-redirect"

  default_url_redirect {
    https_redirect         = true
    redirect_response_code = "MOVED_PERMANENTLY_DEFAULT"
    strip_query            = false
  }
}

# HTTPS proxy
resource "google_compute_target_https_proxy" "main" {
  name             = "omni-${var.customer_name}-https-proxy"
  url_map          = google_compute_url_map.main.id
  ssl_certificates = [google_compute_managed_ssl_certificate.main.id]
}

# HTTP proxy (redirect to HTTPS)
resource "google_compute_target_http_proxy" "redirect" {
  name    = "omni-${var.customer_name}-http-proxy"
  url_map = google_compute_url_map.http_redirect.id
}

# HTTPS forwarding rule (port 443)
resource "google_compute_global_forwarding_rule" "https" {
  name                  = "omni-${var.customer_name}-https-rule"
  ip_address            = google_compute_global_address.lb.address
  port_range            = "443"
  target                = google_compute_target_https_proxy.main.id
  load_balancing_scheme = "EXTERNAL_MANAGED"
}

# HTTP forwarding rule (port 80 -> redirect to HTTPS)
resource "google_compute_global_forwarding_rule" "http_redirect" {
  name                  = "omni-${var.customer_name}-http-rule"
  ip_address            = google_compute_global_address.lb.address
  port_range            = "80"
  target                = google_compute_target_http_proxy.redirect.id
  load_balancing_scheme = "EXTERNAL_MANAGED"
}
