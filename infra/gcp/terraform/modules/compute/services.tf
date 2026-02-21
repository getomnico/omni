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
    DB_MAX_CONNECTIONS         = var.db_max_connections
    DB_ACQUIRE_TIMEOUT_SECONDS = var.db_acquire_timeout_seconds
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

  # Connectors with only basic env vars (CONNECTOR_MANAGER_URL, PORT, RUST_LOG)
  simple_connectors = {
    slack     = { port = 4002, image = "omni-slack-connector" }
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
          SESSION_COOKIE_NAME     = var.session_cookie_name
          SESSION_DURATION_DAYS   = var.session_duration_days
          OMNI_DOMAIN             = var.custom_domain
          ORIGIN                  = local.app_url
          APP_URL                 = local.app_url
          EMAIL_PROVIDER          = "resend"
          RESEND_API_KEY          = var.resend_api_key
          EMAIL_FROM              = "Omni <noreply@getomni.co>"
          AI_ANSWER_ENABLED       = var.ai_answer_enabled
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
          PORT                       = "3001"
          AI_SERVICE_URL             = local.service_url["ai"]
          SEMANTIC_SEARCH_TIMEOUT_MS = var.semantic_search_timeout_ms
          RAG_CONTEXT_WINDOW         = var.rag_context_window
          RUST_LOG                   = var.rust_log
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
          RUST_LOG       = var.rust_log
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
          EMBEDDING_MODEL                                 = var.embedding_model
          AI_WORKERS                                      = var.ai_workers
          EMBEDDING_MAX_MODEL_LEN                         = var.embedding_max_model_len
          EMBEDDING_BATCH_S3_BUCKET                       = var.batch_bucket_name
          EMBEDDING_BATCH_MIN_DOCUMENTS                   = var.embedding_batch_min_documents
          EMBEDDING_BATCH_MAX_DOCUMENTS                   = var.embedding_batch_max_documents
          EMBEDDING_BATCH_ACCUMULATION_TIMEOUT_SECONDS    = var.embedding_batch_accumulation_timeout_seconds
          EMBEDDING_BATCH_ACCUMULATION_POLL_INTERVAL      = var.embedding_batch_accumulation_poll_interval
          EMBEDDING_BATCH_MONITOR_POLL_INTERVAL           = var.embedding_batch_monitor_poll_interval
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
          GOOGLE_CONNECTOR_URL            = local.service_url["google-conn"]
          SLACK_CONNECTOR_URL             = local.service_url["slack-conn"]
          ATLASSIAN_CONNECTOR_URL         = local.service_url["atlassian-conn"]
          WEB_CONNECTOR_URL               = local.service_url["web-conn"]
          GITHUB_CONNECTOR_URL            = local.service_url["github-conn"]
          HUBSPOT_CONNECTOR_URL           = local.service_url["hubspot-conn"]
          MICROSOFT_CONNECTOR_URL         = local.service_url["microsoft-conn"]
          NOTION_CONNECTOR_URL            = local.service_url["notion-conn"]
          FIREFLIES_CONNECTOR_URL         = local.service_url["fireflies-conn"]
          MAX_CONCURRENT_SYNCS            = var.max_concurrent_syncs
          MAX_CONCURRENT_SYNCS_PER_TYPE   = var.max_concurrent_syncs_per_type
          SCHEDULER_POLL_INTERVAL_SECONDS = var.scheduler_poll_interval_seconds
          STALE_SYNC_TIMEOUT_MINUTES      = var.stale_sync_timeout_minutes
          RUST_LOG                        = var.rust_log
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
# Google Connector Service
# ============================================================================

resource "google_cloud_run_v2_service" "google_connector" {
  name     = "omni-${var.customer_name}-google-conn"
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
      image = "ghcr.io/${var.github_org}/omni/omni-google-connector:latest"

      ports {
        container_port = 4001
      }

      resources {
        limits = {
          cpu    = var.cloud_run_cpu
          memory = var.cloud_run_memory
        }
      }

      dynamic "env" {
        for_each = merge(local.redis_env, {
          PORT                                   = "4001"
          CONNECTOR_MANAGER_URL                  = local.service_url["connector-mgr"]
          RUST_LOG                               = var.rust_log
          AI_SERVICE_URL                         = local.service_url["ai"]
          GOOGLE_WEBHOOK_URL                     = var.google_webhook_url
          GOOGLE_SYNC_INTERVAL_SECONDS           = var.google_sync_interval_seconds
          GOOGLE_MAX_AGE_DAYS                    = var.google_max_age_days
          WEBHOOK_RENEWAL_CHECK_INTERVAL_SECONDS = var.webhook_renewal_check_interval_seconds
        })
        content {
          name  = env.key
          value = env.value
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

resource "google_cloud_run_v2_service_iam_member" "google_connector_invoker" {
  name     = google_cloud_run_v2_service.google_connector.name
  location = var.region
  role     = "roles/run.invoker"
  member   = "serviceAccount:${var.cloud_run_sa_email}"
}

# ============================================================================
# Atlassian Connector Service
# ============================================================================

resource "google_cloud_run_v2_service" "atlassian_connector" {
  name     = "omni-${var.customer_name}-atlassian-conn"
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
      image = "ghcr.io/${var.github_org}/omni/omni-atlassian-connector:latest"

      ports {
        container_port = 4003
      }

      resources {
        limits = {
          cpu    = var.cloud_run_cpu
          memory = var.cloud_run_memory
        }
      }

      dynamic "env" {
        for_each = merge(local.redis_env, {
          PORT                  = "4003"
          CONNECTOR_MANAGER_URL = local.service_url["connector-mgr"]
          RUST_LOG              = var.rust_log
        })
        content {
          name  = env.key
          value = env.value
        }
      }
    }
  }
}

resource "google_cloud_run_v2_service_iam_member" "atlassian_connector_invoker" {
  name     = google_cloud_run_v2_service.atlassian_connector.name
  location = var.region
  role     = "roles/run.invoker"
  member   = "serviceAccount:${var.cloud_run_sa_email}"
}

# ============================================================================
# Web Connector Service
# ============================================================================

resource "google_cloud_run_v2_service" "web_connector" {
  name     = "omni-${var.customer_name}-web-conn"
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
      image = "ghcr.io/${var.github_org}/omni/omni-web-connector:latest"

      ports {
        container_port = 4004
      }

      resources {
        limits = {
          cpu    = var.cloud_run_cpu
          memory = var.cloud_run_memory
        }
      }

      dynamic "env" {
        for_each = merge(local.redis_env, {
          PORT                  = "4004"
          CONNECTOR_MANAGER_URL = local.service_url["connector-mgr"]
          RUST_LOG              = var.rust_log
        })
        content {
          name  = env.key
          value = env.value
        }
      }
    }
  }
}

resource "google_cloud_run_v2_service_iam_member" "web_connector_invoker" {
  name     = google_cloud_run_v2_service.web_connector.name
  location = var.region
  role     = "roles/run.invoker"
  member   = "serviceAccount:${var.cloud_run_sa_email}"
}

# ============================================================================
# Simple Connector Services (via for_each)
# ============================================================================

resource "google_cloud_run_v2_service" "connectors" {
  for_each = local.simple_connectors

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
          RUST_LOG              = var.rust_log
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
  for_each = local.simple_connectors

  name     = google_cloud_run_v2_service.connectors[each.key].name
  location = var.region
  role     = "roles/run.invoker"
  member   = "serviceAccount:${var.cloud_run_sa_email}"
}
