resource "random_password" "database" {
  length  = 48
  special = false
  upper   = true
  lower   = true
  numeric = true
}

resource "google_secret_manager_secret" "database_password" {
  secret_id = "omni-${var.customer_name}-database-password"

  replication {
    auto {}
  }
}

resource "google_secret_manager_secret_version" "database_password" {
  secret      = google_secret_manager_secret.database_password.id
  secret_data = random_password.database.result
}

resource "google_secret_manager_secret" "jina_api_key" {
  secret_id = "omni-${var.customer_name}-jina-api-key"

  replication {
    auto {}
  }
}

resource "google_secret_manager_secret_version" "jina_api_key" {
  secret      = google_secret_manager_secret.jina_api_key.id
  secret_data = var.jina_api_key
}

resource "random_password" "encryption_key" {
  length           = 64
  special          = true
  upper            = true
  lower            = true
  numeric          = true
  override_special = "!@#$%^&*()-_=+[]{}|;:,.<>?"
}

resource "google_secret_manager_secret" "encryption_key" {
  secret_id = "omni-${var.customer_name}-encryption-key"

  replication {
    auto {}
  }
}

resource "google_secret_manager_secret_version" "encryption_key" {
  secret      = google_secret_manager_secret.encryption_key.id
  secret_data = random_password.encryption_key.result
}

resource "random_password" "encryption_salt" {
  length           = 32
  special          = true
  upper            = true
  lower            = true
  numeric          = true
  override_special = "!@#$%^&*()-_=+[]{}|;:,.<>?"
}

resource "google_secret_manager_secret" "encryption_salt" {
  secret_id = "omni-${var.customer_name}-encryption-salt"

  replication {
    auto {}
  }
}

resource "google_secret_manager_secret_version" "encryption_salt" {
  secret      = google_secret_manager_secret.encryption_salt.id
  secret_data = random_password.encryption_salt.result
}

resource "random_password" "session_secret" {
  length           = 64
  special          = true
  upper            = true
  lower            = true
  numeric          = true
  override_special = "!@#$%^&*()-_=+[]{}|;:,.<>?"
}

resource "google_secret_manager_secret" "session_secret" {
  secret_id = "omni-${var.customer_name}-session-secret"

  replication {
    auto {}
  }
}

resource "google_secret_manager_secret_version" "session_secret" {
  secret      = google_secret_manager_secret.session_secret.id
  secret_data = random_password.session_secret.result
}
