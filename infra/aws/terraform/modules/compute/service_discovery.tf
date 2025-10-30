resource "aws_service_discovery_service" "web" {
  name = "web"

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

resource "aws_service_discovery_service" "searcher" {
  name = "searcher"

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

resource "aws_service_discovery_service" "indexer" {
  name = "indexer"

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

resource "aws_service_discovery_service" "ai" {
  name = "ai"

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

resource "aws_service_discovery_service" "google_connector" {
  name = "google-connector"

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
