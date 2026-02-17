resource "google_compute_network" "main" {
  name                    = "omni-${var.customer_name}-vpc"
  auto_create_subnetworks = false
}

resource "google_compute_subnetwork" "private" {
  name                     = "omni-${var.customer_name}-private"
  ip_cidr_range            = cidrsubnet(var.vpc_cidr, 8, 11)
  region                   = var.region
  network                  = google_compute_network.main.id
  private_ip_google_access = true
}

# Cloud NAT for outbound internet access from private subnet
resource "google_compute_router" "main" {
  name    = "omni-${var.customer_name}-router"
  region  = var.region
  network = google_compute_network.main.id
}

resource "google_compute_router_nat" "main" {
  name                               = "omni-${var.customer_name}-nat"
  router                             = google_compute_router.main.name
  region                             = var.region
  nat_ip_allocate_option             = "AUTO_ONLY"
  source_subnetwork_ip_ranges_to_nat = "ALL_SUBNETWORKS_ALL_IP_RANGES"

  log_config {
    enable = true
    filter = "ERRORS_ONLY"
  }
}

# VPC Access Connector for Cloud Run to reach VPC resources (DB, Redis)
resource "google_vpc_access_connector" "main" {
  name          = "omni-${var.customer_name}-vpc"
  region        = var.region
  ip_cidr_range = "10.8.0.0/28"
  network       = google_compute_network.main.name

  min_instances = 2
  max_instances = 3
}

# Private service access for Memorystore
resource "google_compute_global_address" "private_service_range" {
  name          = "omni-${var.customer_name}-private-svc"
  purpose       = "VPC_PEERING"
  address_type  = "INTERNAL"
  prefix_length = 16
  network       = google_compute_network.main.id
}

resource "google_service_networking_connection" "private_service" {
  network                 = google_compute_network.main.id
  service                 = "servicenetworking.googleapis.com"
  reserved_peering_ranges = [google_compute_global_address.private_service_range.name]
}

# Firewall: allow internal traffic within VPC
resource "google_compute_firewall" "allow_internal" {
  name    = "omni-${var.customer_name}-allow-internal"
  network = google_compute_network.main.name

  allow {
    protocol = "tcp"
    ports    = ["0-65535"]
  }

  allow {
    protocol = "udp"
    ports    = ["0-65535"]
  }

  allow {
    protocol = "icmp"
  }

  source_ranges = [google_compute_subnetwork.private.ip_cidr_range, "10.8.0.0/28"]
}

# Firewall: allow GCP health checks
resource "google_compute_firewall" "allow_health_checks" {
  name    = "omni-${var.customer_name}-allow-health-checks"
  network = google_compute_network.main.name

  allow {
    protocol = "tcp"
    ports    = ["5432", "6379"]
  }

  source_ranges = ["130.211.0.0/22", "35.191.0.0/16"]
  target_tags   = ["paradedb"]
}

# Firewall: allow IAP SSH for debugging
resource "google_compute_firewall" "allow_iap_ssh" {
  name    = "omni-${var.customer_name}-allow-iap-ssh"
  network = google_compute_network.main.name

  allow {
    protocol = "tcp"
    ports    = ["22"]
  }

  source_ranges = ["35.235.240.0/20"]
  target_tags   = ["paradedb"]
}

# Firewall: allow Cloud Run (via VPC connector) to reach ParadeDB
resource "google_compute_firewall" "allow_vpc_connector_to_db" {
  name    = "omni-${var.customer_name}-allow-vpc-to-db"
  network = google_compute_network.main.name

  allow {
    protocol = "tcp"
    ports    = ["5432"]
  }

  source_ranges = ["10.8.0.0/28"]
  target_tags   = ["paradedb"]
}
