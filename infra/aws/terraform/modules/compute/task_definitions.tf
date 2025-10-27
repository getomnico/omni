locals {
  database_url = "postgresql://${var.database_username}@${var.database_endpoint}:${var.database_port}/${var.database_name}?sslmode=require"
  redis_url    = "redis://${var.redis_endpoint}:${var.redis_port}"

  common_environment = [
    { name = "DATABASE_HOST", value = var.database_endpoint },
    { name = "DATABASE_PORT", value = tostring(var.database_port) },
    { name = "DATABASE_NAME", value = var.database_name },
    { name = "DATABASE_USERNAME", value = var.database_username },
    { name = "DATABASE_SSL", value = "true" },
    { name = "REDIS_URL", value = local.redis_url },
    { name = "DB_MAX_CONNECTIONS", value = "10" },
    { name = "DB_ACQUIRE_TIMEOUT_SECONDS", value = "30" },
    { name = "RUST_LOG", value = "debug" },
    { name = "OTEL_EXPORTER_OTLP_ENDPOINT", value = var.otel_endpoint },
    { name = "OTEL_DEPLOYMENT_ID", value = var.customer_name },
    { name = "OTEL_DEPLOYMENT_ENVIRONMENT", value = "production" },
    { name = "SERVICE_VERSION", value = var.service_version }
  ]

  common_secrets = [
    { name = "DATABASE_PASSWORD", valueFrom = "${var.database_password_arn}:password::" }
  ]
}

# Migrator Task Definition
resource "aws_ecs_task_definition" "migrator" {
  family                   = "omni-${var.customer_name}-migrator"
  network_mode             = "awsvpc"
  requires_compatibilities = ["FARGATE"]
  cpu                      = "256"
  memory                   = "512"
  execution_role_arn       = aws_iam_role.ecs_task_execution.arn
  task_role_arn            = aws_iam_role.ecs_task.arn

  container_definitions = jsonencode([{
    name      = "omni-migrator"
    image     = "ghcr.io/${var.github_org}/omni/omni-migrator:latest"
    essential = true

    logConfiguration = {
      logDriver = "awslogs"
      options = {
        "awslogs-group"         = var.log_group_name
        "awslogs-region"        = var.region
        "awslogs-stream-prefix" = "migrator"
      }
    }

    environment = local.common_environment
    secrets     = local.common_secrets
  }])

  tags = merge(local.common_tags, {
    Name = "omni-${var.customer_name}-migrator"
  })
}

# Web Task Definition
resource "aws_ecs_task_definition" "web" {
  family                   = "omni-${var.customer_name}-web"
  network_mode             = "awsvpc"
  requires_compatibilities = ["FARGATE"]
  cpu                      = var.task_cpu
  memory                   = var.task_memory
  execution_role_arn       = aws_iam_role.ecs_task_execution.arn
  task_role_arn            = aws_iam_role.ecs_task.arn

  container_definitions = jsonencode([{
    name      = "omni-web"
    image     = "ghcr.io/${var.github_org}/omni/omni-web:latest"
    essential = true

    portMappings = [{
      containerPort = 3000
      protocol      = "tcp"
    }]

    logConfiguration = {
      logDriver = "awslogs"
      options = {
        "awslogs-group"         = var.log_group_name
        "awslogs-region"        = var.region
        "awslogs-stream-prefix" = "web"
      }
    }

    environment = concat(local.common_environment, [
      { name = "SEARCHER_URL", value = "http://searcher.omni-${var.customer_name}.local:3001" },
      { name = "INDEXER_URL", value = "http://indexer.omni-${var.customer_name}.local:3002" },
      { name = "AI_SERVICE_URL", value = "http://ai.omni-${var.customer_name}.local:3003" },
      { name = "GOOGLE_CONNECTOR_URL", value = "http://google-connector.omni-${var.customer_name}.local:3004" },
      { name = "SESSION_COOKIE_NAME", value = "omni_session" },
      { name = "SESSION_DURATION_DAYS", value = "30" },
      { name = "ORIGIN", value = local.app_url },
      { name = "APP_URL", value = local.app_url },
      { name = "GOOGLE_CLIENT_ID", value = var.google_client_id },
      { name = "GOOGLE_CLIENT_SECRET", value = var.google_client_secret },
      { name = "GOOGLE_REDIRECT_URI", value = "${local.app_url}/auth/google/callback" },
      { name = "EMAIL_PROVIDER", value = "resend" },
      { name = "RESEND_API_KEY", value = var.resend_api_key },
      { name = "EMAIL_FROM", value = "Omni <noreply@getomni.co>" },
      { name = "AI_ANSWER_ENABLED", value = "true" },
      { name = "AI_FIRST_SEARCH_ENABLED", value = "true" }
    ])

    secrets = concat(local.common_secrets, [
      { name = "SESSION_SECRET", valueFrom = "${var.session_secret_arn}:secret::" }
    ])
  }])

  tags = merge(local.common_tags, {
    Name = "omni-${var.customer_name}-web"
  })
}

# Searcher Task Definition
resource "aws_ecs_task_definition" "searcher" {
  family                   = "omni-${var.customer_name}-searcher"
  network_mode             = "awsvpc"
  requires_compatibilities = ["FARGATE"]
  cpu                      = var.task_cpu
  memory                   = var.task_memory
  execution_role_arn       = aws_iam_role.ecs_task_execution.arn
  task_role_arn            = aws_iam_role.ecs_task.arn

  container_definitions = jsonencode([{
    name      = "omni-searcher"
    image     = "ghcr.io/${var.github_org}/omni/omni-searcher:latest"
    essential = true

    portMappings = [{
      containerPort = 3001
      protocol      = "tcp"
    }]

    logConfiguration = {
      logDriver = "awslogs"
      options = {
        "awslogs-group"         = var.log_group_name
        "awslogs-region"        = var.region
        "awslogs-stream-prefix" = "searcher"
      }
    }

    environment = concat(local.common_environment, [
      { name = "PORT", value = "3001" },
      { name = "AI_SERVICE_URL", value = "http://ai.omni-${var.customer_name}.local:3003" },
      { name = "TYPO_TOLERANCE_ENABLED", value = "true" },
      { name = "TYPO_TOLERANCE_MAX_DISTANCE", value = "2" },
      { name = "TYPO_TOLERANCE_MIN_WORD_LENGTH", value = "4" },
      # Storage configuration
      { name = "STORAGE_BACKEND", value = "s3" },
      { name = "S3_BUCKET", value = var.content_bucket_name },
      { name = "S3_REGION", value = var.region }
    ])

    secrets = local.common_secrets
  }])

  tags = merge(local.common_tags, {
    Name = "omni-${var.customer_name}-searcher"
  })
}

# Indexer Task Definition
resource "aws_ecs_task_definition" "indexer" {
  family                   = "omni-${var.customer_name}-indexer"
  network_mode             = "awsvpc"
  requires_compatibilities = ["FARGATE"]
  cpu                      = var.task_cpu
  memory                   = var.task_memory
  execution_role_arn       = aws_iam_role.ecs_task_execution.arn
  task_role_arn            = aws_iam_role.ecs_task.arn

  container_definitions = jsonencode([{
    name      = "omni-indexer"
    image     = "ghcr.io/${var.github_org}/omni/omni-indexer:latest"
    essential = true

    portMappings = [{
      containerPort = 3002
      protocol      = "tcp"
    }]

    logConfiguration = {
      logDriver = "awslogs"
      options = {
        "awslogs-group"         = var.log_group_name
        "awslogs-region"        = var.region
        "awslogs-stream-prefix" = "indexer"
      }
    }

    environment = concat(local.common_environment, [
      { name = "PORT", value = "3002" },
      { name = "AI_SERVICE_URL", value = "http://ai.omni-${var.customer_name}.local:3003" },
      # Storage configuration
      { name = "STORAGE_BACKEND", value = "s3" },
      { name = "S3_BUCKET", value = var.content_bucket_name },
      { name = "S3_REGION", value = var.region }
    ])

    secrets = concat(local.common_secrets, [
      { name = "ENCRYPTION_KEY", valueFrom = "${var.encryption_key_arn}:key::" },
      { name = "ENCRYPTION_SALT", valueFrom = "${var.encryption_salt_arn}:salt::" }
    ])
  }])

  tags = merge(local.common_tags, {
    Name = "omni-${var.customer_name}-indexer"
  })
}

# AI Task Definition
resource "aws_ecs_task_definition" "ai" {
  family                   = "omni-${var.customer_name}-ai"
  network_mode             = "awsvpc"
  requires_compatibilities = ["FARGATE"]
  cpu                      = var.task_cpu
  memory                   = var.task_memory
  execution_role_arn       = aws_iam_role.ecs_task_execution.arn
  task_role_arn            = aws_iam_role.ecs_task.arn

  container_definitions = jsonencode([{
    name      = "omni-ai"
    image     = "ghcr.io/${var.github_org}/omni/omni-ai:latest"
    essential = true

    command = ["sh", "-c", "python -m uvicorn main:app --host 0.0.0.0 --port $${PORT} --workers $${AI_WORKERS:-1}"]

    portMappings = [{
      containerPort = 3003
      protocol      = "tcp"
    }]

    logConfiguration = {
      logDriver = "awslogs"
      options = {
        "awslogs-group"         = var.log_group_name
        "awslogs-region"        = var.region
        "awslogs-stream-prefix" = "ai"
      }
    }

    environment = concat(local.common_environment, [
      { name = "PORT", value = "3003" },
      { name = "SEARCHER_URL", value = "http://searcher.omni-${var.customer_name}.local:3001" },
      { name = "MODEL_PATH", value = "/models" },
      { name = "EMBEDDING_MODEL", value = "intfloat/e5-large-v2" },
      { name = "EMBEDDING_DIMENSIONS", value = "1024" },
      { name = "EMBEDDING_PROVIDER", value = "jina" },
      { name = "LLM_PROVIDER", value = "bedrock" },
      # { name = "BEDROCK_MODEL_ID", value = "us.anthropic.claude-sonnet-4-5-20250929-v1:0" },
      # { name = "TITLE_GENERATION_MODEL_ID", value = "us.anthropic.claude-haiku-4-5-20251001-v1:0" },
      { name = "BEDROCK_MODEL_ID", value = "amazon.nova-pro-v1:0" },
      { name = "TITLE_GENERATION_MODEL_ID", value = "amazon.nova-lite-v1:0" },
      { name = "ANTHROPIC_MAX_TOKENS", value = "4096" },
      { name = "AI_WORKERS", value = "1" },
      # Storage configuration
      { name = "STORAGE_BACKEND", value = "s3" },
      { name = "S3_BUCKET", value = var.content_bucket_name },
      { name = "S3_REGION", value = var.region },
      # Batch inference configuration
      # TODO: Batch processing on AWS Bedrock is not enabled by default, it requires creating a support case. We will
      # therefore avoid using batch processing for now.
      # We will disable embeddings entirely for now, until we find a good solution for this issue.
      { name = "ENABLE_EMBEDDING_BATCH_INFERENCE", value = "false" },
      { name = "EMBEDDING_BATCH_S3_BUCKET", value = var.batch_bucket_name },
      { name = "EMBEDDING_BATCH_BEDROCK_ROLE_ARN", value = var.bedrock_batch_role_arn },
      { name = "EMBEDDING_BATCH_MIN_DOCUMENTS", value = "100" },
      { name = "EMBEDDING_BATCH_MAX_DOCUMENTS", value = "50000" },
      { name = "EMBEDDING_BATCH_ACCUMULATION_TIMEOUT_SECONDS", value = "300" }
    ])

    secrets = concat(local.common_secrets, [
      { name = "JINA_API_KEY", valueFrom = var.jina_api_key_arn }
    ])
  }])

  tags = merge(local.common_tags, {
    Name = "omni-${var.customer_name}-ai"
  })
}

# Google Connector Task Definition
resource "aws_ecs_task_definition" "google_connector" {
  family                   = "omni-${var.customer_name}-google-connector"
  network_mode             = "awsvpc"
  requires_compatibilities = ["FARGATE"]
  cpu                      = var.task_cpu
  memory                   = var.task_memory
  execution_role_arn       = aws_iam_role.ecs_task_execution.arn
  task_role_arn            = aws_iam_role.ecs_task.arn

  container_definitions = jsonencode([{
    name      = "omni-google-connector"
    image     = "ghcr.io/${var.github_org}/omni/omni-google-connector:latest"
    essential = true

    portMappings = [{
      containerPort = 3004
      protocol      = "tcp"
    }]

    logConfiguration = {
      logDriver = "awslogs"
      options = {
        "awslogs-group"         = var.log_group_name
        "awslogs-region"        = var.region
        "awslogs-stream-prefix" = "google-connector"
      }
    }

    environment = concat(local.common_environment, [
      { name = "PORT", value = "3004" },
      { name = "AI_SERVICE_URL", value = "http://ai.omni-${var.customer_name}.local:3003" },
      { name = "GOOGLE_CLIENT_ID", value = var.google_client_id },
      { name = "GOOGLE_CLIENT_SECRET", value = var.google_client_secret },
      { name = "GOOGLE_REDIRECT_URI", value = "${local.app_url}/auth/google/callback" },
      { name = "GOOGLE_SYNC_INTERVAL_SECONDS", value = "3600" },
      { name = "GOOGLE_MAX_AGE_DAYS", value = "730" },
      { name = "GOOGLE_WEBHOOK_URL", value = "${local.app_url}/api/integrations/google/webhook" },
      { name = "WEBHOOK_RENEWAL_CHECK_INTERVAL_SECONDS", value = "3600" },
      # Storage configuration
      { name = "STORAGE_BACKEND", value = "s3" },
      { name = "S3_BUCKET", value = var.content_bucket_name },
      { name = "S3_REGION", value = var.region }
    ])

    secrets = concat(local.common_secrets, [
      { name = "ENCRYPTION_KEY", valueFrom = "${var.encryption_key_arn}:key::" },
      { name = "ENCRYPTION_SALT", valueFrom = "${var.encryption_salt_arn}:salt::" }
    ])
  }])

  tags = merge(local.common_tags, {
    Name = "omni-${var.customer_name}-google-connector"
  })
}
