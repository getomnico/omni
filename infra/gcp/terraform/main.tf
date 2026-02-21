# Enable required GCP APIs
resource "google_project_service" "apis" {
  for_each = toset([
    "compute.googleapis.com",
    "run.googleapis.com",
    "secretmanager.googleapis.com",
    "redis.googleapis.com",
    "vpcaccess.googleapis.com",
    "servicenetworking.googleapis.com",
    "storage.googleapis.com",
    "logging.googleapis.com",
    "iam.googleapis.com",
  ])

  project = var.project_id
  service = each.value

  disable_dependent_services = false
  disable_on_destroy         = false
}

# Shared service account for Cloud Run services.
# Created at root level to break the circular dependency between compute and storage modules.
resource "google_service_account" "cloud_run" {
  account_id   = "omni-${var.customer_name}-cloud-run"
  display_name = "Omni Cloud Run Service Account"

  depends_on = [google_project_service.apis]
}

module "networking" {
  source = "./modules/networking"

  customer_name = var.customer_name
  environment   = var.environment
  region        = var.region
  vpc_cidr      = var.vpc_cidr

  depends_on = [google_project_service.apis]
}

module "secrets" {
  source = "./modules/secrets"

  customer_name     = var.customer_name
  environment       = var.environment
  database_username = var.database_username
  depends_on        = [google_project_service.apis]
}

module "monitoring" {
  source = "./modules/monitoring"

  customer_name      = var.customer_name
  project_id         = var.project_id
  log_retention_days = var.log_retention_days

  depends_on = [google_project_service.apis]
}

module "storage" {
  source = "./modules/storage"

  customer_name      = var.customer_name
  region             = var.region
  cloud_run_sa_email = google_service_account.cloud_run.email

  depends_on = [google_project_service.apis]
}

module "database" {
  source = "./modules/database"

  customer_name     = var.customer_name
  environment       = var.environment
  region            = var.region
  project_id        = var.project_id
  machine_type      = var.paradedb_machine_type
  disk_size_gb      = var.paradedb_disk_size_gb
  container_image   = var.paradedb_container_image
  database_name     = var.database_name
  database_username = var.database_username
  database_password = module.secrets.database_password
  network_id        = module.networking.network_id
  subnet_id         = module.networking.private_subnet_id
}

module "cache" {
  source = "./modules/cache"

  customer_name              = var.customer_name
  region                     = var.region
  tier                       = var.redis_tier
  memory_size_gb             = var.redis_memory_size_gb
  network_id                 = module.networking.network_id
  private_service_connection = module.networking.private_service_connection
}

module "compute" {
  source = "./modules/compute"

  customer_name = var.customer_name
  environment   = var.environment
  project_id    = var.project_id
  region        = var.region
  github_org    = var.github_org
  custom_domain = var.custom_domain

  cloud_run_sa_email = google_service_account.cloud_run.email
  vpc_connector_id   = module.networking.vpc_connector_id

  cloud_run_cpu           = var.cloud_run_cpu
  cloud_run_memory        = var.cloud_run_memory
  cloud_run_min_instances = var.cloud_run_min_instances
  cloud_run_max_instances = var.cloud_run_max_instances

  database_host     = module.database.internal_ip
  database_port     = module.database.port
  database_name     = var.database_name
  database_username = var.database_username

  redis_url = module.cache.redis_url

  database_password_secret_id = module.secrets.database_password_secret_id
  encryption_key_secret_id    = module.secrets.encryption_key_secret_id
  encryption_salt_secret_id   = module.secrets.encryption_salt_secret_id
  all_secret_ids              = module.secrets.all_secret_ids

  resend_api_key    = var.resend_api_key

  # AI service configuration
  embedding_model         = var.embedding_model
  embedding_max_model_len = var.embedding_max_model_len
  ai_workers              = var.ai_workers

  # Batch embedding configuration
  embedding_batch_min_documents                = var.embedding_batch_min_documents
  embedding_batch_max_documents                = var.embedding_batch_max_documents
  embedding_batch_accumulation_timeout_seconds = var.embedding_batch_accumulation_timeout_seconds
  embedding_batch_accumulation_poll_interval   = var.embedding_batch_accumulation_poll_interval
  embedding_batch_monitor_poll_interval        = var.embedding_batch_monitor_poll_interval

  # Searcher configuration
  semantic_search_timeout_ms = var.semantic_search_timeout_ms
  rag_context_window         = var.rag_context_window

  # Connector manager configuration
  max_concurrent_syncs            = var.max_concurrent_syncs
  max_concurrent_syncs_per_type   = var.max_concurrent_syncs_per_type
  scheduler_poll_interval_seconds = var.scheduler_poll_interval_seconds
  stale_sync_timeout_minutes      = var.stale_sync_timeout_minutes

  # Google connector configuration
  google_webhook_url                     = var.google_webhook_url
  google_sync_interval_seconds           = var.google_sync_interval_seconds
  google_max_age_days                    = var.google_max_age_days
  webhook_renewal_check_interval_seconds = var.webhook_renewal_check_interval_seconds

  # Common service configuration
  rust_log                   = var.rust_log
  db_max_connections         = var.db_max_connections
  db_acquire_timeout_seconds = var.db_acquire_timeout_seconds
  session_cookie_name        = var.session_cookie_name
  session_duration_days      = var.session_duration_days
  ai_answer_enabled          = var.ai_answer_enabled

  content_bucket_name = module.storage.content_bucket_name
  batch_bucket_name   = module.storage.batch_bucket_name
  hmac_access_key     = module.storage.hmac_access_key
  hmac_secret         = module.storage.hmac_secret
}

module "loadbalancer" {
  source = "./modules/loadbalancer"

  customer_name    = var.customer_name
  region           = var.region
  custom_domain    = var.custom_domain
  web_service_name = module.compute.web_service_name
}

module "migrations" {
  source = "./modules/migrations"

  customer_name = var.customer_name
  project_id    = var.project_id
  region        = var.region
  github_org    = var.github_org

  vpc_connector_id   = module.networking.vpc_connector_id
  cloud_run_sa_email = google_service_account.cloud_run.email

  database_host               = module.database.internal_ip
  database_port               = module.database.port
  database_name               = var.database_name
  database_username           = var.database_username
  database_password_secret_id = module.secrets.database_password_secret_id
  redis_url                   = module.cache.redis_url

  depends_on = [
    module.database,
    module.compute,
  ]
}
