locals {
  database_url = "postgresql://${var.database_username}@${var.database_endpoint}:${var.database_port}/${var.database_name}?sslmode=disable"
  redis_url    = "redis://${var.redis_endpoint}:${var.redis_port}"

  connector_manager_url = "http://connector-manager.omni-${var.customer_name}.local:3004"

  otel_environment = [
    { name = "RUST_LOG", value = "info" },
    { name = "OTEL_EXPORTER_OTLP_ENDPOINT", value = var.otel_endpoint },
    { name = "OTEL_DEPLOYMENT_ID", value = var.customer_name },
    { name = "OTEL_DEPLOYMENT_ENVIRONMENT", value = "production" },
    { name = "SERVICE_VERSION", value = var.service_version }
  ]

  redis_environment = [
    { name = "REDIS_URL", value = local.redis_url }
  ]

  db_environment = [
    { name = "DATABASE_HOST", value = var.database_endpoint },
    { name = "DATABASE_PORT", value = tostring(var.database_port) },
    { name = "DATABASE_NAME", value = var.database_name },
    { name = "DATABASE_USERNAME", value = var.database_username },
    { name = "DATABASE_SSL", value = "false" },
    { name = "DB_MAX_CONNECTIONS", value = "10" },
    { name = "DB_ACQUIRE_TIMEOUT_SECONDS", value = "3" }
  ]

  common_environment = concat(local.db_environment, local.redis_environment, local.otel_environment)

  common_secrets = [
    { name = "DATABASE_PASSWORD", valueFrom = "${var.database_password_arn}:password::" }
  ]

  connector_base_environment = concat(local.otel_environment, [
    { name = "CONNECTOR_MANAGER_URL", value = local.connector_manager_url }
  ])
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
      { name = "CONNECTOR_MANAGER_URL", value = local.connector_manager_url },
      { name = "GOOGLE_CONNECTOR_URL", value = "http://google-connector.omni-${var.customer_name}.local:4001" },
      { name = "SLACK_CONNECTOR_URL", value = "http://slack-connector.omni-${var.customer_name}.local:4002" },
      { name = "ATLASSIAN_CONNECTOR_URL", value = "http://atlassian-connector.omni-${var.customer_name}.local:4003" },
      { name = "WEB_CONNECTOR_URL", value = "http://web-connector.omni-${var.customer_name}.local:4004" },
      { name = "SESSION_COOKIE_NAME", value = "auth-session" },
      { name = "SESSION_DURATION_DAYS", value = "7" },
      { name = "OMNI_DOMAIN", value = var.custom_domain },
      { name = "ORIGIN", value = local.app_url },
      { name = "APP_URL", value = local.app_url },
      { name = "EMAIL_PROVIDER", value = "resend" },
      { name = "RESEND_API_KEY", value = var.resend_api_key },
      { name = "EMAIL_FROM", value = "Omni <noreply@getomni.co>" },
      { name = "AI_ANSWER_ENABLED", value = "true" }
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
      { name = "SEMANTIC_SEARCH_TIMEOUT_MS", value = "1000" },
      { name = "RAG_CONTEXT_WINDOW", value = "2" },
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
      { name = "EMBEDDING_PROVIDER", value = "jina" },
      { name = "EMBEDDING_MODEL", value = "jina-embeddings-v3" },
      { name = "EMBEDDING_DIMENSIONS", value = "1024" },
      { name = "EMBEDDING_API_URL", value = var.embedding_api_url },
      { name = "EMBEDDING_MAX_MODEL_LEN", value = "8192" },
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
      { name = "EMBEDDING_API_KEY", valueFrom = var.jina_api_key_arn }
    ])
  }])

  tags = merge(local.common_tags, {
    Name = "omni-${var.customer_name}-ai"
  })
}

# Connector Manager Task Definition
resource "aws_ecs_task_definition" "connector_manager" {
  family                   = "omni-${var.customer_name}-connector-manager"
  network_mode             = "awsvpc"
  requires_compatibilities = ["FARGATE"]
  cpu                      = var.task_cpu
  memory                   = var.task_memory
  execution_role_arn       = aws_iam_role.ecs_task_execution.arn
  task_role_arn            = aws_iam_role.ecs_task.arn

  container_definitions = jsonencode([{
    name      = "omni-connector-manager"
    image     = "ghcr.io/${var.github_org}/omni/omni-connector-manager:latest"
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
        "awslogs-stream-prefix" = "connector-manager"
      }
    }

    environment = concat(local.common_environment, [
      { name = "PORT", value = "3004" },
      { name = "CONNECTOR_GOOGLE_URL", value = "http://google-connector.omni-${var.customer_name}.local:4001" },
      { name = "CONNECTOR_SLACK_URL", value = "http://slack-connector.omni-${var.customer_name}.local:4002" },
      { name = "CONNECTOR_ATLASSIAN_URL", value = "http://atlassian-connector.omni-${var.customer_name}.local:4003" },
      { name = "CONNECTOR_WEB_URL", value = "http://web-connector.omni-${var.customer_name}.local:4004" },
      { name = "CONNECTOR_GITHUB_URL", value = "http://github-connector.omni-${var.customer_name}.local:4005" },
      { name = "CONNECTOR_HUBSPOT_URL", value = "http://hubspot-connector.omni-${var.customer_name}.local:4006" },
      { name = "CONNECTOR_MICROSOFT_URL", value = "http://microsoft-connector.omni-${var.customer_name}.local:4007" },
      { name = "CONNECTOR_NOTION_URL", value = "http://notion-connector.omni-${var.customer_name}.local:4008" },
      { name = "CONNECTOR_FIREFLIES_URL", value = "http://fireflies-connector.omni-${var.customer_name}.local:4009" },
      { name = "MAX_CONCURRENT_SYNCS", value = "10" },
      { name = "MAX_CONCURRENT_SYNCS_PER_TYPE", value = "3" },
      { name = "SCHEDULER_POLL_INTERVAL_SECONDS", value = "60" },
      { name = "STALE_SYNC_TIMEOUT_MINUTES", value = "60" },
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
    Name = "omni-${var.customer_name}-connector-manager"
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
      containerPort = 4001
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

    environment = concat(local.connector_base_environment, [
      { name = "PORT", value = "4001" }
    ])

    secrets = []
  }])

  tags = merge(local.common_tags, {
    Name = "omni-${var.customer_name}-google-connector"
  })
}

# Atlassian Connector Task Definition
resource "aws_ecs_task_definition" "atlassian_connector" {
  family                   = "omni-${var.customer_name}-atlassian-connector"
  network_mode             = "awsvpc"
  requires_compatibilities = ["FARGATE"]
  cpu                      = var.task_cpu
  memory                   = var.task_memory
  execution_role_arn       = aws_iam_role.ecs_task_execution.arn
  task_role_arn            = aws_iam_role.ecs_task.arn

  container_definitions = jsonencode([{
    name      = "omni-atlassian-connector"
    image     = "ghcr.io/${var.github_org}/omni/omni-atlassian-connector:latest"
    essential = true

    portMappings = [{
      containerPort = 4003
      protocol      = "tcp"
    }]

    logConfiguration = {
      logDriver = "awslogs"
      options = {
        "awslogs-group"         = var.log_group_name
        "awslogs-region"        = var.region
        "awslogs-stream-prefix" = "atlassian-connector"
      }
    }

    environment = concat(local.connector_base_environment, [
      { name = "PORT", value = "4003" }
    ])

    secrets = []
  }])

  tags = merge(local.common_tags, {
    Name = "omni-${var.customer_name}-atlassian-connector"
  })
}

# Web Connector Task Definition
resource "aws_ecs_task_definition" "web_connector" {
  family                   = "omni-${var.customer_name}-web-connector"
  network_mode             = "awsvpc"
  requires_compatibilities = ["FARGATE"]
  cpu                      = var.task_cpu
  memory                   = var.task_memory
  execution_role_arn       = aws_iam_role.ecs_task_execution.arn
  task_role_arn            = aws_iam_role.ecs_task.arn

  container_definitions = jsonencode([{
    name      = "omni-web-connector"
    image     = "ghcr.io/${var.github_org}/omni/omni-web-connector:latest"
    essential = true

    portMappings = [{
      containerPort = 4004
      protocol      = "tcp"
    }]

    logConfiguration = {
      logDriver = "awslogs"
      options = {
        "awslogs-group"         = var.log_group_name
        "awslogs-region"        = var.region
        "awslogs-stream-prefix" = "web-connector"
      }
    }

    environment = concat(local.connector_base_environment, [
      { name = "PORT", value = "4004" }
    ])

    secrets = []
  }])

  tags = merge(local.common_tags, {
    Name = "omni-${var.customer_name}-web-connector"
  })
}

# Slack Connector Task Definition
resource "aws_ecs_task_definition" "slack_connector" {
  family                   = "omni-${var.customer_name}-slack-connector"
  network_mode             = "awsvpc"
  requires_compatibilities = ["FARGATE"]
  cpu                      = var.task_cpu
  memory                   = var.task_memory
  execution_role_arn       = aws_iam_role.ecs_task_execution.arn
  task_role_arn            = aws_iam_role.ecs_task.arn

  container_definitions = jsonencode([{
    name      = "omni-slack-connector"
    image     = "ghcr.io/${var.github_org}/omni/omni-slack-connector:latest"
    essential = true

    portMappings = [{
      containerPort = 4002
      protocol      = "tcp"
    }]

    logConfiguration = {
      logDriver = "awslogs"
      options = {
        "awslogs-group"         = var.log_group_name
        "awslogs-region"        = var.region
        "awslogs-stream-prefix" = "slack-connector"
      }
    }

    environment = concat(local.connector_base_environment, [
      { name = "PORT", value = "4002" }
    ])

    secrets = []
  }])

  tags = merge(local.common_tags, {
    Name = "omni-${var.customer_name}-slack-connector"
  })
}

# GitHub Connector Task Definition
resource "aws_ecs_task_definition" "github_connector" {
  family                   = "omni-${var.customer_name}-github-connector"
  network_mode             = "awsvpc"
  requires_compatibilities = ["FARGATE"]
  cpu                      = var.task_cpu
  memory                   = var.task_memory
  execution_role_arn       = aws_iam_role.ecs_task_execution.arn
  task_role_arn            = aws_iam_role.ecs_task.arn

  container_definitions = jsonencode([{
    name      = "omni-github-connector"
    image     = "ghcr.io/${var.github_org}/omni/omni-github-connector:latest"
    essential = true

    portMappings = [{
      containerPort = 4005
      protocol      = "tcp"
    }]

    logConfiguration = {
      logDriver = "awslogs"
      options = {
        "awslogs-group"         = var.log_group_name
        "awslogs-region"        = var.region
        "awslogs-stream-prefix" = "github-connector"
      }
    }

    environment = concat(local.connector_base_environment, [
      { name = "PORT", value = "4005" }
    ])

    secrets = []
  }])

  tags = merge(local.common_tags, {
    Name = "omni-${var.customer_name}-github-connector"
  })
}

# HubSpot Connector Task Definition
resource "aws_ecs_task_definition" "hubspot_connector" {
  family                   = "omni-${var.customer_name}-hubspot-connector"
  network_mode             = "awsvpc"
  requires_compatibilities = ["FARGATE"]
  cpu                      = var.task_cpu
  memory                   = var.task_memory
  execution_role_arn       = aws_iam_role.ecs_task_execution.arn
  task_role_arn            = aws_iam_role.ecs_task.arn

  container_definitions = jsonencode([{
    name      = "omni-hubspot-connector"
    image     = "ghcr.io/${var.github_org}/omni/omni-hubspot-connector:latest"
    essential = true

    portMappings = [{
      containerPort = 4006
      protocol      = "tcp"
    }]

    logConfiguration = {
      logDriver = "awslogs"
      options = {
        "awslogs-group"         = var.log_group_name
        "awslogs-region"        = var.region
        "awslogs-stream-prefix" = "hubspot-connector"
      }
    }

    environment = concat(local.connector_base_environment, [
      { name = "PORT", value = "4006" }
    ])

    secrets = []
  }])

  tags = merge(local.common_tags, {
    Name = "omni-${var.customer_name}-hubspot-connector"
  })
}

# Microsoft Connector Task Definition
resource "aws_ecs_task_definition" "microsoft_connector" {
  family                   = "omni-${var.customer_name}-microsoft-connector"
  network_mode             = "awsvpc"
  requires_compatibilities = ["FARGATE"]
  cpu                      = var.task_cpu
  memory                   = var.task_memory
  execution_role_arn       = aws_iam_role.ecs_task_execution.arn
  task_role_arn            = aws_iam_role.ecs_task.arn

  container_definitions = jsonencode([{
    name      = "omni-microsoft-connector"
    image     = "ghcr.io/${var.github_org}/omni/omni-microsoft-connector:latest"
    essential = true

    portMappings = [{
      containerPort = 4007
      protocol      = "tcp"
    }]

    logConfiguration = {
      logDriver = "awslogs"
      options = {
        "awslogs-group"         = var.log_group_name
        "awslogs-region"        = var.region
        "awslogs-stream-prefix" = "microsoft-connector"
      }
    }

    environment = concat(local.connector_base_environment, [
      { name = "PORT", value = "4007" }
    ])

    secrets = []
  }])

  tags = merge(local.common_tags, {
    Name = "omni-${var.customer_name}-microsoft-connector"
  })
}

# Notion Connector Task Definition
resource "aws_ecs_task_definition" "notion_connector" {
  family                   = "omni-${var.customer_name}-notion-connector"
  network_mode             = "awsvpc"
  requires_compatibilities = ["FARGATE"]
  cpu                      = var.task_cpu
  memory                   = var.task_memory
  execution_role_arn       = aws_iam_role.ecs_task_execution.arn
  task_role_arn            = aws_iam_role.ecs_task.arn

  container_definitions = jsonencode([{
    name      = "omni-notion-connector"
    image     = "ghcr.io/${var.github_org}/omni/omni-notion-connector:latest"
    essential = true

    portMappings = [{
      containerPort = 4008
      protocol      = "tcp"
    }]

    logConfiguration = {
      logDriver = "awslogs"
      options = {
        "awslogs-group"         = var.log_group_name
        "awslogs-region"        = var.region
        "awslogs-stream-prefix" = "notion-connector"
      }
    }

    environment = concat(local.connector_base_environment, [
      { name = "PORT", value = "4008" }
    ])

    secrets = []
  }])

  tags = merge(local.common_tags, {
    Name = "omni-${var.customer_name}-notion-connector"
  })
}

# Fireflies Connector Task Definition
resource "aws_ecs_task_definition" "fireflies_connector" {
  family                   = "omni-${var.customer_name}-fireflies-connector"
  network_mode             = "awsvpc"
  requires_compatibilities = ["FARGATE"]
  cpu                      = var.task_cpu
  memory                   = var.task_memory
  execution_role_arn       = aws_iam_role.ecs_task_execution.arn
  task_role_arn            = aws_iam_role.ecs_task.arn

  container_definitions = jsonencode([{
    name      = "omni-fireflies-connector"
    image     = "ghcr.io/${var.github_org}/omni/omni-fireflies-connector:latest"
    essential = true

    portMappings = [{
      containerPort = 4009
      protocol      = "tcp"
    }]

    logConfiguration = {
      logDriver = "awslogs"
      options = {
        "awslogs-group"         = var.log_group_name
        "awslogs-region"        = var.region
        "awslogs-stream-prefix" = "fireflies-connector"
      }
    }

    environment = concat(local.connector_base_environment, [
      { name = "PORT", value = "4009" }
    ])

    secrets = []
  }])

  tags = merge(local.common_tags, {
    Name = "omni-${var.customer_name}-fireflies-connector"
  })
}
