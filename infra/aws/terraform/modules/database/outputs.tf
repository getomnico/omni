output "endpoint" {
  description = "Database endpoint address"
  value       = var.use_rds ? aws_db_instance.postgresql[0].address : "paradedb.omni-${var.customer_name}.local"
}

output "port" {
  description = "Database port"
  value       = 5432
}

output "database_name" {
  description = "Database name"
  value       = var.database_name
}

output "instance_id" {
  description = "Database instance ID (RDS only)"
  value       = var.use_rds ? aws_db_instance.postgresql[0].id : null
}

output "arn" {
  description = "Database ARN (RDS only)"
  value       = var.use_rds ? aws_db_instance.postgresql[0].arn : null
}
