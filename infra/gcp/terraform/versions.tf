terraform {
  required_version = ">= 1.5.0"

  required_providers {
    google = {
      source  = "hashicorp/google"
      version = "~> 5.0"
    }
    google-beta = {
      source  = "hashicorp/google-beta"
      version = "~> 5.0"
    }
    random = {
      source  = "hashicorp/random"
      version = "~> 3.5"
    }
    null = {
      source  = "hashicorp/null"
      version = "~> 3.2"
    }
  }
}

provider "google" {
  project = var.project_id
  region  = var.region

  default_labels = {
    application = "omni"
    customer    = var.customer_name
    environment = var.environment
    managed-by  = "terraform"
  }
}

provider "google-beta" {
  project = var.project_id
  region  = var.region

  default_labels = {
    application = "omni"
    customer    = var.customer_name
    environment = var.environment
    managed-by  = "terraform"
  }
}
