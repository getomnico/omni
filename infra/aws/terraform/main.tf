data "aws_caller_identity" "current" {}
data "aws_region" "current" {}

module "networking" {
  source = "./modules/networking"

  customer_name = var.customer_name
  environment   = var.environment
  vpc_cidr      = var.vpc_cidr
}

module "secrets" {
  source = "./modules/secrets"

  customer_name     = var.customer_name
  environment       = var.environment
  database_username = var.database_username
}

module "monitoring" {
  source = "./modules/monitoring"

  customer_name      = var.customer_name
  environment        = var.environment
  log_retention_days = var.log_retention_days
}

# ECS Cluster and Service Discovery (created early for ParadeDB dependency)
resource "aws_ecs_cluster" "main" {
  name = "omni-${var.customer_name}-cluster"

  setting {
    name  = "containerInsights"
    value = "enabled"
  }

  tags = {
    Application = "Omni"
    Customer    = var.customer_name
    Environment = var.environment
    ManagedBy   = "Terraform"
  }
}

resource "aws_service_discovery_private_dns_namespace" "main" {
  name        = "omni-${var.customer_name}.local"
  description = "Private DNS namespace for Omni services"
  vpc         = module.networking.vpc_id

  tags = {
    Application = "Omni"
    Customer    = var.customer_name
    Environment = var.environment
    ManagedBy   = "Terraform"
  }
}

module "database" {
  source = "./modules/database"

  customer_name = var.customer_name
  environment   = var.environment

  # Database variables
  database_name     = var.database_name
  database_username = var.database_username
  subnet_ids        = module.networking.private_subnet_ids
  region            = var.region

  # ParadeDB configuration
  paradedb_instance_type         = var.paradedb_instance_type
  paradedb_volume_size           = var.paradedb_volume_size
  paradedb_container_image       = var.paradedb_container_image
  vpc_id                         = module.networking.vpc_id
  ecs_security_group_id          = module.networking.ecs_security_group_id
  database_password_secret_arn   = module.secrets.database_password_arn
  ecs_cluster_name               = aws_ecs_cluster.main.name
  service_discovery_namespace_id = aws_service_discovery_private_dns_namespace.main.id
}

# Attach ParadeDB capacity provider to cluster
resource "aws_ecs_cluster_capacity_providers" "main" {
  cluster_name = aws_ecs_cluster.main.name

  capacity_providers = [module.database.paradedb_capacity_provider_name]

  depends_on = [module.database]
}

module "cache" {
  source = "./modules/cache"

  customer_name  = var.customer_name
  environment    = var.environment
  node_type      = var.redis_node_type
  engine_version = var.redis_engine_version

  subnet_ids        = module.networking.private_subnet_ids
  security_group_id = module.networking.redis_security_group_id
}

module "loadbalancer" {
  source = "./modules/loadbalancer"

  customer_name     = var.customer_name
  environment       = var.environment
  vpc_id            = module.networking.vpc_id
  subnet_ids        = module.networking.public_subnet_ids
  security_group_id = module.networking.alb_security_group_id

  ssl_certificate_arn        = var.ssl_certificate_arn
  enable_deletion_protection = var.alb_deletion_protection
}

module "storage" {
  source = "./modules/storage"

  customer_name = var.customer_name
  tags = {
    Environment = var.environment
    ManagedBy   = "Terraform"
    Customer    = var.customer_name
  }
}

module "compute" {
  source = "./modules/compute"

  customer_name     = var.customer_name
  environment       = var.environment
  github_org        = var.github_org
  vpc_id            = module.networking.vpc_id
  subnet_ids        = module.networking.private_subnet_ids
  security_group_id = module.networking.ecs_security_group_id

  # Pass existing cluster and namespace
  cluster_name                   = aws_ecs_cluster.main.name
  cluster_arn                    = aws_ecs_cluster.main.arn
  service_discovery_namespace_id = aws_service_discovery_private_dns_namespace.main.id

  alb_target_group_arn = module.loadbalancer.target_group_arn
  alb_dns_name         = module.loadbalancer.dns_name
  custom_domain        = var.custom_domain

  task_cpu      = var.ecs_task_cpu
  task_memory   = var.ecs_task_memory
  desired_count = var.ecs_desired_count

  database_endpoint = module.database.endpoint
  database_port     = module.database.port
  database_name     = module.database.database_name
  database_username = var.database_username

  redis_endpoint = module.cache.endpoint
  redis_port     = module.cache.port

  log_group_name = module.monitoring.log_group_name
  region         = var.region

  database_password_arn = module.secrets.database_password_arn
  encryption_key_arn    = module.secrets.encryption_key_arn
  encryption_salt_arn   = module.secrets.encryption_salt_arn

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

  # Storage resources for S3 and batch inference
  content_bucket_arn     = module.storage.content_bucket_arn
  content_bucket_name    = module.storage.content_bucket_name
  batch_bucket_arn       = module.storage.batch_bucket_arn
  batch_bucket_name      = module.storage.batch_bucket_name
  bedrock_batch_role_arn = module.storage.bedrock_batch_role_arn
}

module "migrations" {
  source = "./modules/migrations"

  customer_name = var.customer_name
  environment   = var.environment

  cluster_name                 = module.compute.cluster_name
  migrator_task_definition_arn = module.compute.migrator_task_definition_arn
  subnet_ids                   = module.networking.private_subnet_ids
  security_group_id            = module.networking.ecs_security_group_id
  region                       = var.region

  task_execution_role_arn = module.compute.task_execution_role_arn
  task_role_arn           = module.compute.task_role_arn

  depends_on = [
    module.database,
    module.compute
  ]
}
