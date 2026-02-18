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
          var.encryption_salt_arn
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
      Statement = [
        {
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
            "arn:aws:bedrock:*::foundation-model/amazon.*",
            "arn:aws:bedrock:*::foundation-model/amazon.titan-embed-text-*"
          ]
        },
        {
          Effect = "Allow"
          Action = [
            "bedrock:CreateModelInvocationJob",
            "bedrock:GetModelInvocationJob",
            "bedrock:ListModelInvocationJobs",
            "bedrock:StopModelInvocationJob"
          ]
          Resource = "*"
        },
        {
          Effect   = "Allow"
          Action   = ["iam:PassRole"]
          Resource = var.bedrock_batch_role_arn
          Condition = {
            StringEquals = {
              "iam:PassedToService" = "bedrock.amazonaws.com"
            }
          }
        }
      ]
    })
  }

  inline_policy {
    name = "S3Access"
    policy = jsonencode({
      Version = "2012-10-17"
      Statement = [{
        Effect = "Allow"
        Action = [
          "s3:GetObject",
          "s3:PutObject",
          "s3:DeleteObject",
          "s3:ListBucket"
        ]
        Resource = [
          var.content_bucket_arn,
          "${var.content_bucket_arn}/*",
          var.batch_bucket_arn,
          "${var.batch_bucket_arn}/*"
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

  inline_policy {
    name = "MarketplaceAccess"
    policy = jsonencode({
      Version = "2012-10-17"
      Statement = [{
        Effect = "Allow"
        Action = [
          "aws-marketplace:ViewSubscriptions",
          "aws-marketplace:Subscribe",
          "aws-marketplace:Unsubscribe"
        ]
        Resource = "*"
      }]
    })
  }

  tags = merge(local.common_tags, {
    Name = "omni-${var.customer_name}-task-role"
  })
}
