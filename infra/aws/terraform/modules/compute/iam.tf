resource "aws_iam_role" "ecs_task_execution" {
  name = "omni-${var.customer_name}-execution-role"

  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect = "Allow"
      Principal = {
        Service = "ecs-tasks.amazonaws.com"
      }
      Action = "sts:AssumeRole"
    }]
  })

  managed_policy_arns = [
    "arn:aws:iam::aws:policy/service-role/AmazonECSTaskExecutionRolePolicy"
  ]

  inline_policy {
    name = "SecretsManagerAccess"
    policy = jsonencode({
      Version = "2012-10-17"
      Statement = [{
        Effect = "Allow"
        Action = [
          "secretsmanager:GetSecretValue"
        ]
        Resource = [
          var.database_password_arn,
          var.jina_api_key_arn,
          var.encryption_key_arn,
          var.encryption_salt_arn,
          var.session_secret_arn
        ]
      }]
    })
  }

  tags = merge(local.common_tags, {
    Name = "omni-${var.customer_name}-execution-role"
  })
}

resource "aws_iam_role" "ecs_task" {
  name = "omni-${var.customer_name}-task-role"

  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect = "Allow"
      Principal = {
        Service = "ecs-tasks.amazonaws.com"
      }
      Action = "sts:AssumeRole"
    }]
  })

  inline_policy {
    name = "BedrockAccess"
    policy = jsonencode({
      Version = "2012-10-17"
      Statement = [{
        Effect = "Allow"
        Action = [
          "bedrock:InvokeModel",
          "bedrock:InvokeModelWithResponseStream"
        ]
        Resource = [
          "arn:aws:bedrock:*:*:inference-profile/us.anthropic.*",
          "arn:aws:bedrock:*:*:inference-profile/eu.anthropic.*",
          "arn:aws:bedrock:*::foundation-model/anthropic.*",
          "arn:aws:bedrock:*:*:inference-profile/us.amazon.*",
          "arn:aws:bedrock:*:*:inference-profile/eu.amazon.*",
          "arn:aws:bedrock:*::foundation-model/amazon.*"
        ]
      }]
    })
  }

  inline_policy {
    name = "ECSExecAccess"
    policy = jsonencode({
      Version = "2012-10-17"
      Statement = [
        {
          Effect = "Allow"
          Action = [
            "ssmmessages:CreateControlChannel",
            "ssmmessages:CreateDataChannel",
            "ssmmessages:OpenControlChannel",
            "ssmmessages:OpenDataChannel"
          ]
          Resource = "*"
        },
        {
          Effect = "Allow"
          Action = [
            "logs:CreateLogStream",
            "logs:DescribeLogStreams",
            "logs:PutLogEvents"
          ]
          Resource = "*"
        }
      ]
    })
  }

  tags = merge(local.common_tags, {
    Name = "omni-${var.customer_name}-task-role"
  })
}
