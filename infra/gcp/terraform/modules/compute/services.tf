data "google_project" "current" {}

locals {
  app_url        = "https://${var.custom_domain}"
  project_number = data.google_project.current.number

  # Deterministic Cloud Run URL: https://{service-name}-{project-number}.{region}.run.app
  service_url = { for name in [
    "web", "searcher", "indexer", "ai", "connector-mgr",
    "google-conn", "slack-conn", "atlassian-conn", "web-conn",
    "github-conn", "hubspot-conn", "microsoft-conn", "notion-conn", "fireflies-conn",
  ] : name => "https://omni-${var.customer_name}-${name}-${local.project_number}.${var.region}.run.app" }

  db_env = {
    DATABASE_HOST              = var.database_host
    DATABASE_PORT              = tostring(var.database_port)
    DATABASE_NAME              = var.database_name
    DATABASE_USERNAME          = var.database_username
    DATABASE_SSL               = "false"
    DB_MAX_CONNECTIONS         = "10"
    DB_ACQUIRE_TIMEOUT_SECONDS = "3"
  }

  redis_env = {
    REDIS_URL = var.redis_url
  }

  storage_env = {
    STORAGE_BACKEND       = "s3"
    S3_BUCKET             = var.content_bucket_name
    S3_REGION             = var.region
    S3_ENDPOINT           = "https://storage.googleapis.com"
    AWS_ACCESS_KEY_ID     = var.hmac_access_key
    AWS_SECRET_ACCESS_KEY = var.hmac_secret
  }

  common_env = merge(local.db_env, local.redis_env)

  connectors = {
    google    = { port = 4001, image = "omni-google-connector" }
    slack     = { port = 4002, image = "omni-slack-connector" }
    atlassian = { port = 4003, image = "omni-atlassian-connector" }
    web       = { port = 4004, image = "omni-web-connector" }
    github    = { port = 4005, image = "omni-github-connector" }
    hubspot   = { port = 4006, image = "omni-hubspot-connector" }
    microsoft = { port = 4007, image = "omni-microsoft-connector" }
    notion    = { port = 4008, image = "omni-notion-connector" }
    fireflies = { port = 4009, image = "omni-fireflies-connector" }
  }
}

# ============================================================================
# Web Service
# ============================================================================

resource "google_cloud_run_v2_service" "web" {
  name     = "omni-${var.customer_name}-web"
  location = var.region
  ingress  = "INGRESS_TRAFFIC_ALL"

  template {
    service_account = var.cloud_run_sa_email

    scaling {
      min_instance_count = var.cloud_run_min_instances
      max_instance_count = var.cloud_run_max_instances
    }

    vpc_access {
      connector = var.vpc_connector_id
      egress    = "PRIVATE_RANGES_ONLY"
    }

    containers {
      image = "ghcr.io/${var.github_org}/omni/omni-web:latest"

      ports {
        container_port = 3000
      }

      resources {
        limits = {
          cpu    = var.cloud_run_cpu
          memory = var.cloud_run_memory
        }
      }

      dynamic "env" {
        for_each = merge(local.common_env, {
          SEARCHER_URL            = local.service_url["searcher"]
          INDEXER_URL             = local.service_url["indexer"]
          AI_SERVICE_URL          = local.service_url["ai"]
          CONNECTOR_MANAGER_URL   = local.service_url["connector-mgr"]
          GOOGLE_CONNECTOR_URL    = local.service_url["google-conn"]
          SLACK_CONNECTOR_URL     = local.service_url["slack-conn"]
          ATLASSIAN_CONNECTOR_URL = local.service_url["atlassian-conn"]
          WEB_CONNECTOR_URL       = local.service_url["web-conn"]
          SESSION_COOKIE_NAME     = "auth-session"
          SESSION_DURATION_DAYS   = "7"
          OMNI_DOMAIN             = var.custom_domain
          ORIGIN                  = local.app_url
          APP_URL                 = local.app_url
          EMAIL_PROVIDER          = "resend"
          RESEND_API_KEY          = var.resend_api_key
          EMAIL_FROM              = "Omni <noreply@getomni.co>"
          AI_ANSWER_ENABLED       = "true"
        })
        content {
          name  = env.key
          value = env.value
        }
      }

      env {
        name = "DATABASE_PASSWORD"
        value_source {
          secret_key_ref {
            secret  = var.database_password_secret_id
            version = "latest"
          }
        }
      }

      env {
        name = "SESSION_SECRET"
        value_source {
          secret_key_ref {
            secret  = var.session_secret_secret_id
            version = "latest"
          }
        }
      }
    }
  }

  depends_on = [
    google_secret_manager_secret_iam_member.cloud_run_secret_access,
  ]
}

resource "google_cloud_run_v2_service_iam_member" "web_public" {
  name     = google_cloud_run_v2_service.web.name
  location = var.region
  role     = "roles/run.invoker"
  member   = "allUsers"
}

# ============================================================================
# Searcher Service
# ============================================================================

resource "google_cloud_run_v2_service" "searcher" {
  name     = "omni-${var.customer_name}-searcher"
  location = var.region
  ingress  = "INGRESS_TRAFFIC_INTERNAL_ONLY"

  template {
    service_account = var.cloud_run_sa_email

    scaling {
      min_instance_count = var.cloud_run_min_instances
      max_instance_count = var.cloud_run_max_instances
    }

    vpc_access {
      connector = var.vpc_connector_id
      egress    = "PRIVATE_RANGES_ONLY"
    }

    containers {
      image = "ghcr.io/${var.github_org}/omni/omni-searcher:latest"

      ports {
        container_port = 3001
      }

      resources {
        limits = {
          cpu    = var.cloud_run_cpu
          memory = var.cloud_run_memory
        }
      }

      dynamic "env" {
        for_each = merge(local.common_env, local.storage_env, {
          PORT                           = "3001"
          AI_SERVICE_URL                 = local.service_url["ai"]
          TYPO_TOLERANCE_ENABLED         = "true"
          TYPO_TOLERANCE_MAX_DISTANCE    = "2"
          TYPO_TOLERANCE_MIN_WORD_LENGTH = "4"
          SEMANTIC_SEARCH_TIMEOUT_MS     = "1000"
          RAG_CONTEXT_WINDOW             = "2"
          RUST_LOG                       = "info"
        })
        content {
          name  = env.key
          value = env.value
        }
      }

      env {
        name = "DATABASE_PASSWORD"
        value_source {
          secret_key_ref {
            secret  = var.database_password_secret_id
            version = "latest"
          }
        }
      }
    }
  }

  depends_on = [
    google_secret_manager_secret_iam_member.cloud_run_secret_access,
  ]
}

resource "google_cloud_run_v2_service_iam_member" "searcher_invoker" {
  name     = google_cloud_run_v2_service.searcher.name
  location = var.region
  role     = "roles/run.invoker"
  member   = "serviceAccount:${var.cloud_run_sa_email}"
}

# ============================================================================
# Indexer Service
# ============================================================================

resource "google_cloud_run_v2_service" "indexer" {
  name     = "omni-${var.customer_name}-indexer"
  location = var.region
  ingress  = "INGRESS_TRAFFIC_INTERNAL_ONLY"

  template {
    service_account = var.cloud_run_sa_email

    scaling {
      min_instance_count = var.cloud_run_min_instances
      max_instance_count = var.cloud_run_max_instances
    }

    vpc_access {
      connector = var.vpc_connector_id
      egress    = "PRIVATE_RANGES_ONLY"
    }

    containers {
      image = "ghcr.io/${var.github_org}/omni/omni-indexer:latest"

      ports {
        container_port = 3002
      }

      resources {
        limits = {
          cpu    = var.cloud_run_cpu
          memory = var.cloud_run_memory
        }
      }

      dynamic "env" {
        for_each = merge(local.common_env, local.storage_env, {
          PORT           = "3002"
          AI_SERVICE_URL = local.service_url["ai"]
          RUST_LOG       = "info"
        })
        content {
          name  = env.key
          value = env.value
        }
      }

      env {
        name = "DATABASE_PASSWORD"
        value_source {
          secret_key_ref {
            secret  = var.database_password_secret_id
            version = "latest"
          }
        }
      }

      env {
        name = "ENCRYPTION_KEY"
        value_source {
          secret_key_ref {
            secret  = var.encryption_key_secret_id
            version = "latest"
          }
        }
      }

      env {
        name = "ENCRYPTION_SALT"
        value_source {
          secret_key_ref {
            secret  = var.encryption_salt_secret_id
            version = "latest"
          }
        }
      }
    }
  }

  depends_on = [
    google_secret_manager_secret_iam_member.cloud_run_secret_access,
  ]
}

resource "google_cloud_run_v2_service_iam_member" "indexer_invoker" {
  name     = google_cloud_run_v2_service.indexer.name
  location = var.region
  role     = "roles/run.invoker"
  member   = "serviceAccount:${var.cloud_run_sa_email}"
}

# ============================================================================
# AI Service
# ============================================================================

resource "google_cloud_run_v2_service" "ai" {
  name     = "omni-${var.customer_name}-ai"
  location = var.region
  ingress  = "INGRESS_TRAFFIC_INTERNAL_ONLY"

  template {
    service_account = var.cloud_run_sa_email

    scaling {
      min_instance_count = var.cloud_run_min_instances
      max_instance_count = var.cloud_run_max_instances
    }

    vpc_access {
      connector = var.vpc_connector_id
      egress    = "PRIVATE_RANGES_ONLY"
    }

    containers {
      image   = "ghcr.io/${var.github_org}/omni/omni-ai:latest"
      command = ["sh", "-c", "python -m uvicorn main:app --host 0.0.0.0 --port $${PORT} --workers $${AI_WORKERS:-1}"]

      ports {
        container_port = 3003
      }

      resources {
        limits = {
          cpu    = var.cloud_run_cpu
          memory = var.cloud_run_memory
        }
      }

      dynamic "env" {
        for_each = merge(local.common_env, local.storage_env, {
          PORT                             = "3003"
          SEARCHER_URL                     = local.service_url["searcher"]
          MODEL_PATH                       = "/models"
          EMBEDDING_PROVIDER               = "jina"
          EMBEDDING_MODEL                  = "jina-embeddings-v3"
          EMBEDDING_DIMENSIONS             = "1024"
          AI_WORKERS                       = "1"
          EMBEDDING_API_URL                = var.embedding_api_url
          EMBEDDING_MAX_MODEL_LEN          = "8192"
          ENABLE_EMBEDDING_BATCH_INFERENCE = "false"
          EMBEDDING_BATCH_S3_BUCKET        = var.batch_bucket_name
        })
        content {
          name  = env.key
          value = env.value
        }
      }

      env {
        name = "DATABASE_PASSWORD"
        value_source {
          secret_key_ref {
            secret  = var.database_password_secret_id
            version = "latest"
          }
        }
      }

      env {
        name = "EMBEDDING_API_KEY"
        value_source {
          secret_key_ref {
            secret  = var.jina_api_key_secret_id
            version = "latest"
          }
        }
      }

    }
  }

  depends_on = [
    google_secret_manager_secret_iam_member.cloud_run_secret_access,
  ]
}

resource "google_cloud_run_v2_service_iam_member" "ai_invoker" {
  name     = google_cloud_run_v2_service.ai.name
  location = var.region
  role     = "roles/run.invoker"
  member   = "serviceAccount:${var.cloud_run_sa_email}"
}

# ============================================================================
# Connector Manager Service
# ============================================================================

resource "google_cloud_run_v2_service" "connector_manager" {
  name     = "omni-${var.customer_name}-connector-mgr"
  location = var.region
  ingress  = "INGRESS_TRAFFIC_INTERNAL_ONLY"

  template {
    service_account = var.cloud_run_sa_email

    scaling {
      min_instance_count = var.cloud_run_min_instances
      max_instance_count = var.cloud_run_max_instances
    }

    vpc_access {
      connector = var.vpc_connector_id
      egress    = "PRIVATE_RANGES_ONLY"
    }

    containers {
      image = "ghcr.io/${var.github_org}/omni/omni-connector-manager:latest"

      ports {
        container_port = 3004
      }

      resources {
        limits = {
          cpu    = var.cloud_run_cpu
          memory = var.cloud_run_memory
        }
      }

      dynamic "env" {
        for_each = merge(local.common_env, local.storage_env, {
          PORT                            = "3004"
          CONNECTOR_GOOGLE_URL            = local.service_url["google-conn"]
          CONNECTOR_SLACK_URL             = local.service_url["slack-conn"]
          CONNECTOR_ATLASSIAN_URL         = local.service_url["atlassian-conn"]
          CONNECTOR_WEB_URL               = local.service_url["web-conn"]
          CONNECTOR_GITHUB_URL            = local.service_url["github-conn"]
          CONNECTOR_HUBSPOT_URL           = local.service_url["hubspot-conn"]
          CONNECTOR_MICROSOFT_URL         = local.service_url["microsoft-conn"]
          CONNECTOR_NOTION_URL            = local.service_url["notion-conn"]
          CONNECTOR_FIREFLIES_URL         = local.service_url["fireflies-conn"]
          MAX_CONCURRENT_SYNCS            = "10"
          MAX_CONCURRENT_SYNCS_PER_TYPE   = "3"
          SCHEDULER_POLL_INTERVAL_SECONDS = "60"
          STALE_SYNC_TIMEOUT_MINUTES      = "60"
          RUST_LOG                        = "info"
        })
        content {
          name  = env.key
          value = env.value
        }
      }

      env {
        name = "DATABASE_PASSWORD"
        value_source {
          secret_key_ref {
            secret  = var.database_password_secret_id
            version = "latest"
          }
        }
      }

      env {
        name = "ENCRYPTION_KEY"
        value_source {
          secret_key_ref {
            secret  = var.encryption_key_secret_id
            version = "latest"
          }
        }
      }

      env {
        name = "ENCRYPTION_SALT"
        value_source {
          secret_key_ref {
            secret  = var.encryption_salt_secret_id
            version = "latest"
          }
        }
      }
    }
  }

  depends_on = [
    google_secret_manager_secret_iam_member.cloud_run_secret_access,
  ]
}

resource "google_cloud_run_v2_service_iam_member" "connector_manager_invoker" {
  name     = google_cloud_run_v2_service.connector_manager.name
  location = var.region
  role     = "roles/run.invoker"
  member   = "serviceAccount:${var.cloud_run_sa_email}"
}

# ============================================================================
# Connector Services (all 9 connectors via for_each)
# ============================================================================

resource "google_cloud_run_v2_service" "connectors" {
  for_each = local.connectors

  name     = "omni-${var.customer_name}-${each.key}-conn"
  location = var.region
  ingress  = "INGRESS_TRAFFIC_INTERNAL_ONLY"

  template {
    service_account = var.cloud_run_sa_email

    scaling {
      min_instance_count = var.cloud_run_min_instances
      max_instance_count = var.cloud_run_max_instances
    }

    vpc_access {
      connector = var.vpc_connector_id
      egress    = "PRIVATE_RANGES_ONLY"
    }

    containers {
      image = "ghcr.io/${var.github_org}/omni/${each.value.image}:latest"

      ports {
        container_port = each.value.port
      }

      resources {
        limits = {
          cpu    = var.cloud_run_cpu
          memory = var.cloud_run_memory
        }
      }

      dynamic "env" {
        for_each = {
          PORT                  = tostring(each.value.port)
          CONNECTOR_MANAGER_URL = local.service_url["connector-mgr"]
          RUST_LOG              = "info"
        }
        content {
          name  = env.key
          value = env.value
        }
      }
    }
  }
}

resource "google_cloud_run_v2_service_iam_member" "connector_invoker" {
  for_each = local.connectors

  name     = google_cloud_run_v2_service.connectors[each.key].name
  location = var.region
  role     = "roles/run.invoker"
  member   = "serviceAccount:${var.cloud_run_sa_email}"
}
