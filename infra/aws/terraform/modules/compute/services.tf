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
  name            = "omni-${var.customer_name}-google-connector"
  cluster         = var.cluster_arn
  task_definition = aws_ecs_task_definition.google_connector.arn
  launch_type     = "FARGATE"
  desired_count   = var.desired_count

  enable_execute_command = true

  network_configuration {
    security_groups  = [var.security_group_id]
    subnets          = var.subnet_ids
    assign_public_ip = false
  }

  service_registries {
    registry_arn = aws_service_discovery_service.google_connector.arn
  }

  tags = merge(local.common_tags, {
    Name = "omni-${var.customer_name}-google-connector"
  })
}
