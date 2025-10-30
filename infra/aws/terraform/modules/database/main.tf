locals {
  common_tags = {
    Application = "Omni"
    Customer    = var.customer_name
    Environment = var.environment
    ManagedBy   = "Terraform"
  }
}

# ============================================================================
# RDS PostgreSQL Resources (when use_rds = true)
# ============================================================================

resource "aws_db_subnet_group" "main" {
  count = var.use_rds ? 1 : 0

  name        = "omni-${var.customer_name}-db-subnet-group"
  description = "Subnet group for RDS database"
  subnet_ids  = var.subnet_ids

  tags = merge(local.common_tags, {
    Name = "omni-${var.customer_name}-db-subnet-group"
  })
}

resource "aws_db_instance" "postgresql" {
  count = var.use_rds ? 1 : 0

  identifier     = "omni-${var.customer_name}-postgres"
  engine         = "postgres"
  engine_version = "17.4"

  instance_class    = var.instance_class
  allocated_storage = var.allocated_storage
  storage_type      = "gp3"
  storage_encrypted = true

  db_name  = var.database_name
  username = var.database_username
  password = var.database_password

  db_subnet_group_name   = aws_db_subnet_group.main[0].name
  vpc_security_group_ids = [var.security_group_id]

  backup_retention_period = var.backup_retention_period
  backup_window           = "03:00-04:00"
  maintenance_window      = "sun:04:00-sun:05:00"

  multi_az               = var.multi_az
  deletion_protection    = var.deletion_protection
  skip_final_snapshot    = var.skip_final_snapshot
  final_snapshot_identifier = var.skip_final_snapshot ? null : "omni-${var.customer_name}-postgres-final-${formatdate("YYYY-MM-DD-hhmm", timestamp())}"

  enabled_cloudwatch_logs_exports = ["postgresql", "upgrade"]

  ca_cert_identifier = "rds-ca-rsa2048-g1"

  tags = merge(local.common_tags, {
    Name = "omni-${var.customer_name}-postgres"
  })
}

# ============================================================================
# ParadeDB on ECS Resources (when use_rds = false)
# ============================================================================

# Get latest ECS-optimized AMI
data "aws_ami" "ecs_optimized" {
  count = var.use_rds ? 0 : 1

  most_recent = true
  owners      = ["amazon"]

  filter {
    name   = "name"
    values = ["amzn2-ami-ecs-hvm-*-x86_64-ebs"]
  }

  filter {
    name   = "virtualization-type"
    values = ["hvm"]
  }
}

# Security group for ParadeDB
resource "aws_security_group" "paradedb" {
  count = var.use_rds ? 0 : 1

  name        = "omni-${var.customer_name}-paradedb-sg"
  description = "Security group for ParadeDB database"
  vpc_id      = var.vpc_id

  ingress {
    description     = "PostgreSQL from ECS services"
    from_port       = 5432
    to_port         = 5432
    protocol        = "tcp"
    security_groups = [var.ecs_security_group_id]
  }

  egress {
    description = "Allow all outbound traffic"
    from_port   = 0
    to_port     = 0
    protocol    = "-1"
    cidr_blocks = ["0.0.0.0/0"]
  }

  tags = merge(local.common_tags, {
    Name = "omni-${var.customer_name}-paradedb-sg"
  })
}

# IAM role for ParadeDB EC2 instance
resource "aws_iam_role" "paradedb_instance" {
  count = var.use_rds ? 0 : 1

  name = "omni-${var.customer_name}-paradedb-instance-role"

  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Action = "sts:AssumeRole"
      Effect = "Allow"
      Principal = {
        Service = "ec2.amazonaws.com"
      }
    }]
  })

  tags = local.common_tags
}

resource "aws_iam_role_policy_attachment" "paradedb_ecs_instance" {
  count = var.use_rds ? 0 : 1

  role       = aws_iam_role.paradedb_instance[0].name
  policy_arn = "arn:aws:iam::aws:policy/service-role/AmazonEC2ContainerServiceforEC2Role"
}

resource "aws_iam_instance_profile" "paradedb" {
  count = var.use_rds ? 0 : 1

  name = "omni-${var.customer_name}-paradedb-instance-profile"
  role = aws_iam_role.paradedb_instance[0].name

  tags = local.common_tags
}

# Launch template for ParadeDB EC2 instances
resource "aws_launch_template" "paradedb" {
  count = var.use_rds ? 0 : 1

  name_prefix   = "omni-${var.customer_name}-paradedb-"
  image_id      = data.aws_ami.ecs_optimized[0].id
  instance_type = var.paradedb_instance_type

  iam_instance_profile {
    name = aws_iam_instance_profile.paradedb[0].name
  }

  vpc_security_group_ids = [aws_security_group.paradedb[0].id]

  block_device_mappings {
    device_name = "/dev/xvda"
    ebs {
      volume_size           = 30
      volume_type           = "gp3"
      encrypted             = true
      delete_on_termination = true
    }
  }

  # Additional EBS volume for PostgreSQL data
  block_device_mappings {
    device_name = "/dev/xvdf"
    ebs {
      volume_size           = var.paradedb_volume_size
      volume_type           = "gp3"
      encrypted             = true
      delete_on_termination = false
    }
  }

  user_data = base64encode(templatefile("${path.module}/user_data.sh", {
    cluster_name = var.ecs_cluster_name
    device_name  = "/dev/xvdf"
    mount_point  = "/data/postgres"
  }))

  tag_specifications {
    resource_type = "instance"
    tags = merge(local.common_tags, {
      Name = "omni-${var.customer_name}-paradedb"
    })
  }

  tag_specifications {
    resource_type = "volume"
    tags = merge(local.common_tags, {
      Name = "omni-${var.customer_name}-paradedb-volume"
    })
  }

  tags = local.common_tags
}

# Auto Scaling Group for ParadeDB
resource "aws_autoscaling_group" "paradedb" {
  count = var.use_rds ? 0 : 1

  name                = "omni-${var.customer_name}-paradedb-asg"
  vpc_zone_identifier = var.subnet_ids
  min_size            = 1
  max_size            = 1
  desired_capacity    = 1

  launch_template {
    id      = aws_launch_template.paradedb[0].id
    version = "$Latest"
  }

  health_check_type         = "EC2"
  health_check_grace_period = 300

  tag {
    key                 = "Name"
    value               = "omni-${var.customer_name}-paradedb"
    propagate_at_launch = true
  }

  tag {
    key                 = "AmazonECSManaged"
    value               = "true"
    propagate_at_launch = true
  }

  dynamic "tag" {
    for_each = local.common_tags
    content {
      key                 = tag.key
      value               = tag.value
      propagate_at_launch = true
    }
  }
}

# ECS Capacity Provider for ParadeDB
resource "aws_ecs_capacity_provider" "paradedb" {
  count = var.use_rds ? 0 : 1

  name = "omni-${var.customer_name}-paradedb-capacity-provider"

  auto_scaling_group_provider {
    auto_scaling_group_arn         = aws_autoscaling_group.paradedb[0].arn
    managed_termination_protection = "DISABLED"

    managed_scaling {
      status          = "ENABLED"
      target_capacity = 100
    }
  }

  tags = local.common_tags
}

# CloudWatch Log Group for ParadeDB
resource "aws_cloudwatch_log_group" "paradedb" {
  count = var.use_rds ? 0 : 1

  name              = "/ecs/omni-${var.customer_name}/paradedb"
  retention_in_days = 7

  tags = local.common_tags
}

# IAM role for ParadeDB ECS task execution
resource "aws_iam_role" "paradedb_task_execution" {
  count = var.use_rds ? 0 : 1

  name = "omni-${var.customer_name}-paradedb-task-execution-role"

  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Action = "sts:AssumeRole"
      Effect = "Allow"
      Principal = {
        Service = "ecs-tasks.amazonaws.com"
      }
    }]
  })

  tags = local.common_tags
}

resource "aws_iam_role_policy_attachment" "paradedb_task_execution" {
  count = var.use_rds ? 0 : 1

  role       = aws_iam_role.paradedb_task_execution[0].name
  policy_arn = "arn:aws:iam::aws:policy/service-role/AmazonECSTaskExecutionRolePolicy"
}

resource "aws_iam_role_policy" "paradedb_secrets" {
  count = var.use_rds ? 0 : 1

  name = "omni-${var.customer_name}-paradedb-secrets-policy"
  role = aws_iam_role.paradedb_task_execution[0].id

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect = "Allow"
      Action = [
        "secretsmanager:GetSecretValue"
      ]
      Resource = var.database_password_secret_arn
    }]
  })
}

# ECS Task Definition for ParadeDB
resource "aws_ecs_task_definition" "paradedb" {
  count = var.use_rds ? 0 : 1

  family                   = "omni-${var.customer_name}-paradedb"
  network_mode             = "awsvpc"
  requires_compatibilities = ["EC2"]
  cpu                      = "1024"
  memory                   = "2048"
  execution_role_arn       = aws_iam_role.paradedb_task_execution[0].arn

  volume {
    name      = "postgres-data"
    host_path = "/data/postgres"
  }

  container_definitions = jsonencode([{
    name      = "paradedb"
    image     = var.paradedb_container_image
    essential = true

    portMappings = [{
      containerPort = 5432
      hostPort      = 5432
      protocol      = "tcp"
    }]

    environment = [
      {
        name  = "POSTGRES_DB"
        value = var.database_name
      },
      {
        name  = "POSTGRES_USER"
        value = var.database_username
      }
    ]

    secrets = [{
      name      = "POSTGRES_PASSWORD"
      valueFrom = var.database_password_secret_arn
    }]

    mountPoints = [{
      sourceVolume  = "postgres-data"
      containerPath = "/var/lib/postgresql/data"
      readOnly      = false
    }]

    healthCheck = {
      command = [
        "CMD-SHELL",
        "pg_isready -U ${var.database_username} -d ${var.database_name}"
      ]
      interval    = 30
      timeout     = 5
      retries     = 3
      startPeriod = 60
    }

    logConfiguration = {
      logDriver = "awslogs"
      options = {
        "awslogs-group"         = aws_cloudwatch_log_group.paradedb[0].name
        "awslogs-region"        = var.region
        "awslogs-stream-prefix" = "paradedb"
      }
    }
  }])

  tags = local.common_tags
}

# Service Discovery for ParadeDB
resource "aws_service_discovery_service" "paradedb" {
  count = var.use_rds ? 0 : 1

  name = "paradedb"

  dns_config {
    namespace_id = var.service_discovery_namespace_id

    dns_records {
      ttl  = 300
      type = "A"
    }
  }

  health_check_custom_config {
    failure_threshold = 1
  }
}

# ECS Service for ParadeDB
resource "aws_ecs_service" "paradedb" {
  count = var.use_rds ? 0 : 1

  name            = "omni-${var.customer_name}-paradedb"
  cluster         = var.ecs_cluster_name
  task_definition = aws_ecs_task_definition.paradedb[0].arn
  desired_count   = 1
  launch_type     = "EC2"

  network_configuration {
    subnets          = var.subnet_ids
    security_groups  = [aws_security_group.paradedb[0].id]
    assign_public_ip = false
  }

  service_registries {
    registry_arn = aws_service_discovery_service.paradedb[0].arn
  }

  # Ensure only one task per instance
  placement_constraints {
    type = "distinctInstance"
  }

  # Ensure task runs on instances with the right capacity provider
  capacity_provider_strategy {
    capacity_provider = aws_ecs_capacity_provider.paradedb[0].name
    weight            = 100
    base              = 1
  }

  # Enable ECS Exec for debugging
  enable_execute_command = true

  tags = local.common_tags

  depends_on = [
    aws_autoscaling_group.paradedb,
    aws_ecs_capacity_provider.paradedb
  ]
}
