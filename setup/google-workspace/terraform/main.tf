terraform {
  required_version = ">= 1.0"
  required_providers {
    google = {
      source  = "hashicorp/google"
      version = "~> 5.0"
    }
    google-beta = {
      source  = "hashicorp/google-beta"
      version = "~> 5.0"
    }
    time = {
      source  = "hashicorp/time"
      version = "~> 0.9"
    }
  }
}

# Configure the Google Cloud Provider
provider "google" {
  project = var.project_id
  region  = var.region
}

provider "google-beta" {
  project = var.project_id
  region  = var.region
}

# Data sources
data "google_organization" "org" {
  domain = var.workspace_domain
}

data "google_billing_account" "account" {
  display_name = var.billing_account_name
  open         = true
}

data "google_client_config" "default" {}

# Generate unique project ID
resource "random_id" "project_suffix" {
  byte_length = 4
}

locals {
  project_id = var.project_id != "" ? var.project_id : "${var.project_name}-${random_id.project_suffix.hex}"

  required_apis = [
    "admin.googleapis.com",
    "drive.googleapis.com",
    "gmail.googleapis.com",
    "docs.googleapis.com",
    "sheets.googleapis.com",
    "slides.googleapis.com",
    "cloudresourcemanager.googleapis.com",
    "iam.googleapis.com",
    "orgpolicy.googleapis.com"
  ]

  oauth_scopes = concat(
    [
      "https://www.googleapis.com/auth/admin.directory.user.readonly",
      "https://www.googleapis.com/auth/admin.directory.group.readonly",
      "https://www.googleapis.com/auth/drive.readonly"
    ],
    var.include_gmail_scope ? ["https://www.googleapis.com/auth/gmail.readonly"] : []
  )
}

# Create the project
resource "google_project" "omni_project" {
  name                = var.project_name
  project_id          = local.project_id
  org_id              = data.google_organization.org.org_id
  billing_account     = data.google_billing_account.account.id
  auto_create_network = false

  labels = {
    environment = "production"
    purpose     = "omni-integration"
    managed-by  = "terraform"
  }
}

# Enable required APIs
resource "google_project_service" "required_apis" {
  for_each = toset(local.required_apis)

  project = google_project.omni_project.project_id
  service = each.value

  disable_dependent_services = false
  disable_on_destroy         = false
}

# Wait for APIs to be fully enabled
resource "time_sleep" "wait_for_apis" {
  depends_on = [google_project_service.required_apis]

  create_duration = "60s"
}

# Create tag key at organization level (only when managing org policy)
resource "google_tags_tag_key" "omni_integration" {
  count    = var.manage_org_policy ? 1 : 0
  provider = google-beta

  parent      = "organizations/${data.google_organization.org.org_id}"
  short_name  = var.tag_key_name
  description = "Tag for Omni workspace integration projects"

  depends_on = [time_sleep.wait_for_apis]
}

# Create tag value
resource "google_tags_tag_value" "allowed" {
  count    = var.manage_org_policy ? 1 : 0
  provider = google-beta

  parent      = google_tags_tag_key.omni_integration[0].id
  short_name  = var.tag_value_name
  description = "Allowed value for Omni integration"
}

# Attach tag to project
resource "google_tags_tag_binding" "project_tag" {
  count    = var.manage_org_policy ? 1 : 0
  provider = google-beta

  parent    = "//cloudresourcemanager.googleapis.com/projects/${google_project.omni_project.number}"
  tag_value = google_tags_tag_value.allowed[0].id
}

# Organization policy to allow service account key creation for tagged projects
resource "google_org_policy_policy" "service_account_key_policy" {
  count    = var.manage_org_policy ? 1 : 0
  provider = google-beta

  name   = "organizations/${data.google_organization.org.org_id}/policies/iam.disableServiceAccountKeyCreation"
  parent = "organizations/${data.google_organization.org.org_id}"

  spec {
    rules {
      allow_all = "TRUE"
      condition {
        expression  = "resource.matchTagId(\"${google_tags_tag_key.omni_integration[0].namespaced_name}\", \"${google_tags_tag_value.allowed[0].namespaced_name}\")"
        title       = "Omni Integration Exception"
        description = "Allow service account key creation for Omni workspace integration"
      }
    }

    rules {
      deny_all = "TRUE"
    }
  }

  depends_on = [
    google_tags_tag_binding.project_tag,
    time_sleep.wait_for_apis
  ]
}

# Wait for organization policy to propagate
resource "time_sleep" "wait_for_policy" {
  count      = var.manage_org_policy ? 1 : 0
  depends_on = [google_org_policy_policy.service_account_key_policy]

  create_duration = "30s"
}

# Create service account
resource "google_service_account" "omni_sa" {
  project = google_project.omni_project.project_id

  account_id   = var.service_account_name
  display_name = "Omni Workspace Integration"
  description  = "Service account for Omni workspace integration with read-only access to Google Workspace data"

  depends_on = [time_sleep.wait_for_apis]
}

# Create service account key
resource "google_service_account_key" "omni_sa_key" {
  service_account_id = google_service_account.omni_sa.name

  depends_on = [time_sleep.wait_for_policy]
  # When manage_org_policy=false, wait_for_policy has count=0, so the
  # depends_on resolves to an empty set and the key is created immediately.
}

# Grant minimal IAM roles to service account (optional - OAuth scopes provide the main authorization)
resource "google_project_iam_member" "sa_minimal_permissions" {
  for_each = toset([
    "roles/iam.serviceAccountTokenCreator"
  ])

  project = google_project.omni_project.project_id
  role    = each.value
  member  = "serviceAccount:${google_service_account.omni_sa.email}"
}

# Save the service account key to a local file
resource "local_file" "service_account_key" {
  count = var.output_key_file ? 1 : 0

  content         = base64decode(google_service_account_key.omni_sa_key.private_key)
  filename        = "${path.module}/omni-service-account-key.json"
  file_permission = "0600"
}

# Create a summary file with setup instructions
resource "local_file" "setup_instructions" {
  content = templatefile("${path.module}/templates/setup_instructions.tpl", {
    project_id        = google_project.omni_project.project_id
    service_account   = google_service_account.omni_sa.email
    client_id         = google_service_account.omni_sa.unique_id
    oauth_scopes      = join(",\n     ", local.oauth_scopes)
    admin_email       = var.admin_email
    workspace_domain  = var.workspace_domain
    key_file_created  = var.output_key_file
    manage_org_policy = var.manage_org_policy
  })
  filename = "${path.module}/SETUP_INSTRUCTIONS.md"
}
