# Web Service
resource "aws_ecs_service" "web" {
  name            = "omni-${var.customer_name}-web"
  cluster         = var.cluster_arn
  task_definition = aws_ecs_task_definition.web.arn
  launch_type     = "FARGATE"
  desired_count   = var.desired_count

  enable_execute_command = true

  network_configuration {
    security_groups  = [var.security_group_id]
    subnets          = var.subnet_ids
    assign_public_ip = false
  }

  load_balancer {
    target_group_arn = var.alb_target_group_arn
    container_name   = "omni-web"
    container_port   = 3000
  }

  service_registries {
    registry_arn = aws_service_discovery_service.web.arn
  }

  tags = merge(local.common_tags, {
    Name = "omni-${var.customer_name}-web"
  })
}

# Searcher Service
resource "aws_ecs_service" "searcher" {
  name            = "omni-${var.customer_name}-searcher"
  cluster         = var.cluster_arn
  task_definition = aws_ecs_task_definition.searcher.arn
  launch_type     = "FARGATE"
  desired_count   = var.desired_count

  enable_execute_command = true

  network_configuration {
    security_groups  = [var.security_group_id]
    subnets          = var.subnet_ids
    assign_public_ip = false
  }

  service_registries {
    registry_arn = aws_service_discovery_service.searcher.arn
  }

  tags = merge(local.common_tags, {
    Name = "omni-${var.customer_name}-searcher"
  })
}

# Indexer Service
resource "aws_ecs_service" "indexer" {
  name            = "omni-${var.customer_name}-indexer"
  cluster         = var.cluster_arn
  task_definition = aws_ecs_task_definition.indexer.arn
  launch_type     = "FARGATE"
  desired_count   = var.desired_count

  enable_execute_command = true

  network_configuration {
    security_groups  = [var.security_group_id]
    subnets          = var.subnet_ids
    assign_public_ip = false
  }

  service_registries {
    registry_arn = aws_service_discovery_service.indexer.arn
  }

  tags = merge(local.common_tags, {
    Name = "omni-${var.customer_name}-indexer"
  })
}

# AI Service
resource "aws_ecs_service" "ai" {
  name            = "omni-${var.customer_name}-ai"
  cluster         = var.cluster_arn
  task_definition = aws_ecs_task_definition.ai.arn
  launch_type     = "FARGATE"
  desired_count   = var.desired_count

  enable_execute_command = true

  network_configuration {
    security_groups  = [var.security_group_id]
    subnets          = var.subnet_ids
    assign_public_ip = false
  }

  service_registries {
    registry_arn = aws_service_discovery_service.ai.arn
  }

  tags = merge(local.common_tags, {
    Name = "omni-${var.customer_name}-ai"
  })
}

# Google Connector Service
resource "aws_ecs_service" "google_connector" {
  count = contains(var.enabled_connectors, "google") ? 1 : 0

  name            = "omni-${var.customer_name}-google-connector"
  cluster         = var.cluster_arn
  task_definition = aws_ecs_task_definition.google_connector[0].arn
  launch_type     = "FARGATE"
  desired_count   = var.desired_count

  enable_execute_command = true

  network_configuration {
    security_groups  = [var.security_group_id]
    subnets          = var.subnet_ids
    assign_public_ip = false
  }

  service_registries {
    registry_arn = aws_service_discovery_service.google_connector[0].arn
  }

  tags = merge(local.common_tags, {
    Name = "omni-${var.customer_name}-google-connector"
  })
}

# Atlassian Connector Service
resource "aws_ecs_service" "atlassian_connector" {
  count = contains(var.enabled_connectors, "atlassian") ? 1 : 0

  name            = "omni-${var.customer_name}-atlassian-connector"
  cluster         = var.cluster_arn
  task_definition = aws_ecs_task_definition.atlassian_connector[0].arn
  launch_type     = "FARGATE"
  desired_count   = var.desired_count

  enable_execute_command = true

  network_configuration {
    security_groups  = [var.security_group_id]
    subnets          = var.subnet_ids
    assign_public_ip = false
  }

  service_registries {
    registry_arn = aws_service_discovery_service.atlassian_connector[0].arn
  }

  tags = merge(local.common_tags, {
    Name = "omni-${var.customer_name}-atlassian-connector"
  })
}

# Web Connector Service
resource "aws_ecs_service" "web_connector" {
  count = contains(var.enabled_connectors, "web") ? 1 : 0

  name            = "omni-${var.customer_name}-web-connector"
  cluster         = var.cluster_arn
  task_definition = aws_ecs_task_definition.web_connector[0].arn
  launch_type     = "FARGATE"
  desired_count   = var.desired_count

  enable_execute_command = true

  network_configuration {
    security_groups  = [var.security_group_id]
    subnets          = var.subnet_ids
    assign_public_ip = false
  }

  service_registries {
    registry_arn = aws_service_discovery_service.web_connector[0].arn
  }

  tags = merge(local.common_tags, {
    Name = "omni-${var.customer_name}-web-connector"
  })
}

# Connector Manager Service
resource "aws_ecs_service" "connector_manager" {
  name            = "omni-${var.customer_name}-connector-manager"
  cluster         = var.cluster_arn
  task_definition = aws_ecs_task_definition.connector_manager.arn
  launch_type     = "FARGATE"
  desired_count   = var.desired_count

  enable_execute_command = true

  network_configuration {
    security_groups  = [var.security_group_id]
    subnets          = var.subnet_ids
    assign_public_ip = false
  }

  service_registries {
    registry_arn = aws_service_discovery_service.connector_manager.arn
  }

  tags = merge(local.common_tags, {
    Name = "omni-${var.customer_name}-connector-manager"
  })
}

# Slack Connector Service
resource "aws_ecs_service" "slack_connector" {
  count = contains(var.enabled_connectors, "slack") ? 1 : 0

  name            = "omni-${var.customer_name}-slack-connector"
  cluster         = var.cluster_arn
  task_definition = aws_ecs_task_definition.slack_connector[0].arn
  launch_type     = "FARGATE"
  desired_count   = var.desired_count

  enable_execute_command = true

  network_configuration {
    security_groups  = [var.security_group_id]
    subnets          = var.subnet_ids
    assign_public_ip = false
  }

  service_registries {
    registry_arn = aws_service_discovery_service.slack_connector[0].arn
  }

  tags = merge(local.common_tags, {
    Name = "omni-${var.customer_name}-slack-connector"
  })
}

# GitHub Connector Service
resource "aws_ecs_service" "github_connector" {
  count = contains(var.enabled_connectors, "github") ? 1 : 0

  name            = "omni-${var.customer_name}-github-connector"
  cluster         = var.cluster_arn
  task_definition = aws_ecs_task_definition.github_connector[0].arn
  launch_type     = "FARGATE"
  desired_count   = var.desired_count

  enable_execute_command = true

  network_configuration {
    security_groups  = [var.security_group_id]
    subnets          = var.subnet_ids
    assign_public_ip = false
  }

  service_registries {
    registry_arn = aws_service_discovery_service.github_connector[0].arn
  }

  tags = merge(local.common_tags, {
    Name = "omni-${var.customer_name}-github-connector"
  })
}

# HubSpot Connector Service
resource "aws_ecs_service" "hubspot_connector" {
  count = contains(var.enabled_connectors, "hubspot") ? 1 : 0

  name            = "omni-${var.customer_name}-hubspot-connector"
  cluster         = var.cluster_arn
  task_definition = aws_ecs_task_definition.hubspot_connector[0].arn
  launch_type     = "FARGATE"
  desired_count   = var.desired_count

  enable_execute_command = true

  network_configuration {
    security_groups  = [var.security_group_id]
    subnets          = var.subnet_ids
    assign_public_ip = false
  }

  service_registries {
    registry_arn = aws_service_discovery_service.hubspot_connector[0].arn
  }

  tags = merge(local.common_tags, {
    Name = "omni-${var.customer_name}-hubspot-connector"
  })
}

# Microsoft Connector Service
resource "aws_ecs_service" "microsoft_connector" {
  count = contains(var.enabled_connectors, "microsoft") ? 1 : 0

  name            = "omni-${var.customer_name}-microsoft-connector"
  cluster         = var.cluster_arn
  task_definition = aws_ecs_task_definition.microsoft_connector[0].arn
  launch_type     = "FARGATE"
  desired_count   = var.desired_count

  enable_execute_command = true

  network_configuration {
    security_groups  = [var.security_group_id]
    subnets          = var.subnet_ids
    assign_public_ip = false
  }

  service_registries {
    registry_arn = aws_service_discovery_service.microsoft_connector[0].arn
  }

  tags = merge(local.common_tags, {
    Name = "omni-${var.customer_name}-microsoft-connector"
  })
}

# Notion Connector Service
resource "aws_ecs_service" "notion_connector" {
  count = contains(var.enabled_connectors, "notion") ? 1 : 0

  name            = "omni-${var.customer_name}-notion-connector"
  cluster         = var.cluster_arn
  task_definition = aws_ecs_task_definition.notion_connector[0].arn
  launch_type     = "FARGATE"
  desired_count   = var.desired_count

  enable_execute_command = true

  network_configuration {
    security_groups  = [var.security_group_id]
    subnets          = var.subnet_ids
    assign_public_ip = false
  }

  service_registries {
    registry_arn = aws_service_discovery_service.notion_connector[0].arn
  }

  tags = merge(local.common_tags, {
    Name = "omni-${var.customer_name}-notion-connector"
  })
}

# Fireflies Connector Service
resource "aws_ecs_service" "fireflies_connector" {
  count = contains(var.enabled_connectors, "fireflies") ? 1 : 0

  name            = "omni-${var.customer_name}-fireflies-connector"
  cluster         = var.cluster_arn
  task_definition = aws_ecs_task_definition.fireflies_connector[0].arn
  launch_type     = "FARGATE"
  desired_count   = var.desired_count

  enable_execute_command = true

  network_configuration {
    security_groups  = [var.security_group_id]
    subnets          = var.subnet_ids
    assign_public_ip = false
  }

  service_registries {
    registry_arn = aws_service_discovery_service.fireflies_connector[0].arn
  }

  tags = merge(local.common_tags, {
    Name = "omni-${var.customer_name}-fireflies-connector"
  })
}

# IMAP Connector Service
resource "aws_ecs_service" "imap_connector" {
  count = contains(var.enabled_connectors, "imap") ? 1 : 0

  name            = "omni-${var.customer_name}-imap-connector"
  cluster         = var.cluster_arn
  task_definition = aws_ecs_task_definition.imap_connector[0].arn
  launch_type     = "FARGATE"
  desired_count   = var.desired_count

  enable_execute_command = true

  network_configuration {
    security_groups  = [var.security_group_id]
    subnets          = var.subnet_ids
    assign_public_ip = false
  }

  service_registries {
    registry_arn = aws_service_discovery_service.imap_connector[0].arn
  }

  tags = merge(local.common_tags, {
    Name = "omni-${var.customer_name}-imap-connector"
  })
}

# Linear Connector Service
resource "aws_ecs_service" "linear_connector" {
  count = contains(var.enabled_connectors, "linear") ? 1 : 0

  name            = "omni-${var.customer_name}-linear-connector"
  cluster         = var.cluster_arn
  task_definition = aws_ecs_task_definition.linear_connector[0].arn
  launch_type     = "FARGATE"
  desired_count   = var.desired_count

  enable_execute_command = true

  network_configuration {
    security_groups  = [var.security_group_id]
    subnets          = var.subnet_ids
    assign_public_ip = false
  }

  service_registries {
    registry_arn = aws_service_discovery_service.linear_connector[0].arn
  }

  tags = merge(local.common_tags, {
    Name = "omni-${var.customer_name}-linear-connector"
  })
}

# ClickUp Connector Service
resource "aws_ecs_service" "clickup_connector" {
  count = contains(var.enabled_connectors, "clickup") ? 1 : 0

  name            = "omni-${var.customer_name}-clickup-connector"
  cluster         = var.cluster_arn
  task_definition = aws_ecs_task_definition.clickup_connector[0].arn
  launch_type     = "FARGATE"
  desired_count   = var.desired_count

  enable_execute_command = true

  network_configuration {
    security_groups  = [var.security_group_id]
    subnets          = var.subnet_ids
    assign_public_ip = false
  }

  service_registries {
    registry_arn = aws_service_discovery_service.clickup_connector[0].arn
  }

  tags = merge(local.common_tags, {
    Name = "omni-${var.customer_name}-clickup-connector"
  })
}

# Filesystem Connector Service
resource "aws_ecs_service" "filesystem_connector" {
  count = contains(var.enabled_connectors, "filesystem") ? 1 : 0

  name            = "omni-${var.customer_name}-filesystem-connector"
  cluster         = var.cluster_arn
  task_definition = aws_ecs_task_definition.filesystem_connector[0].arn
  launch_type     = "FARGATE"
  desired_count   = var.desired_count

  enable_execute_command = true

  network_configuration {
    security_groups  = [var.security_group_id]
    subnets          = var.subnet_ids
    assign_public_ip = false
  }

  service_registries {
    registry_arn = aws_service_discovery_service.filesystem_connector[0].arn
  }

  tags = merge(local.common_tags, {
    Name = "omni-${var.customer_name}-filesystem-connector"
  })
}
