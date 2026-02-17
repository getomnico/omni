locals {
  zone = var.zone != "" ? var.zone : "${var.region}-a"
}

resource "google_service_account" "paradedb" {
  account_id   = "omni-${var.customer_name}-paradedb"
  display_name = "Omni ParadeDB VM Service Account"
}

resource "google_project_iam_member" "paradedb_log_writer" {
  project = var.project_id
  role    = "roles/logging.logWriter"
  member  = "serviceAccount:${google_service_account.paradedb.email}"
}

# Static internal IP for stable DATABASE_HOST
resource "google_compute_address" "paradedb" {
  name         = "omni-${var.customer_name}-paradedb-ip"
  subnetwork   = var.subnet_id
  address_type = "INTERNAL"
  region       = var.region
}

# Persistent SSD for PostgreSQL data
resource "google_compute_disk" "paradedb_data" {
  name = "omni-${var.customer_name}-paradedb-data"
  type = "pd-ssd"
  zone = local.zone
  size = var.disk_size_gb
}

resource "google_compute_instance" "paradedb" {
  name         = "omni-${var.customer_name}-paradedb"
  machine_type = var.machine_type
  zone         = local.zone

  tags = ["paradedb"]

  boot_disk {
    initialize_params {
      image = "projects/cos-cloud/global/images/family/cos-stable"
      size  = 30
      type  = "pd-ssd"
    }
  }

  attached_disk {
    source      = google_compute_disk.paradedb_data.id
    device_name = "paradedb-data"
    mode        = "READ_WRITE"
  }

  network_interface {
    subnetwork = var.subnet_id
    network_ip = google_compute_address.paradedb.address
  }

  metadata = {
    gce-container-declaration = yamlencode({
      spec = {
        containers = [{
          image = var.container_image
          env = [
            { name = "POSTGRES_DB", value = var.database_name },
            { name = "POSTGRES_USER", value = var.database_username },
            { name = "POSTGRES_PASSWORD", value = var.database_password },
          ]
          volumeMounts = [{
            name      = "postgres-data"
            mountPath = "/var/lib/postgresql/data"
          }]
        }]
        volumes = [{
          name = "postgres-data"
          hostPath = {
            path = "/mnt/disks/paradedb-data"
          }
        }]
        restartPolicy = "Always"
      }
    })

    # Startup script to format and mount the data disk
    startup-script = <<-EOT
      #!/bin/bash
      set -e
      DEVICE="/dev/disk/by-id/google-paradedb-data"
      MOUNT_POINT="/mnt/disks/paradedb-data"

      if [ ! -d "$MOUNT_POINT" ]; then
        mkdir -p "$MOUNT_POINT"
      fi

      # Format only if not already formatted
      if ! blkid "$DEVICE"; then
        mkfs.ext4 -F "$DEVICE"
      fi

      mount -o discard,defaults "$DEVICE" "$MOUNT_POINT"
      chmod 777 "$MOUNT_POINT"
    EOT
  }

  service_account {
    email  = google_service_account.paradedb.email
    scopes = ["cloud-platform"]
  }

  scheduling {
    automatic_restart   = true
    on_host_maintenance = "MIGRATE"
  }

  allow_stopping_for_update = true
}
