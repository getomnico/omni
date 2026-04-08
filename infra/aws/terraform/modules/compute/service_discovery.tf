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
  count = contains(var.enabled_connectors, "google") ? 1 : 0

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

resource "aws_service_discovery_service" "atlassian_connector" {
  count = contains(var.enabled_connectors, "atlassian") ? 1 : 0

  name = "atlassian-connector"

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

resource "aws_service_discovery_service" "web_connector" {
  count = contains(var.enabled_connectors, "web") ? 1 : 0

  name = "web-connector"

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

resource "aws_service_discovery_service" "connector_manager" {
  name = "connector-manager"

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

resource "aws_service_discovery_service" "slack_connector" {
  count = contains(var.enabled_connectors, "slack") ? 1 : 0

  name = "slack-connector"

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

resource "aws_service_discovery_service" "github_connector" {
  count = contains(var.enabled_connectors, "github") ? 1 : 0

  name = "github-connector"

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

resource "aws_service_discovery_service" "hubspot_connector" {
  count = contains(var.enabled_connectors, "hubspot") ? 1 : 0

  name = "hubspot-connector"

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

resource "aws_service_discovery_service" "microsoft_connector" {
  count = contains(var.enabled_connectors, "microsoft") ? 1 : 0

  name = "microsoft-connector"

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

resource "aws_service_discovery_service" "notion_connector" {
  count = contains(var.enabled_connectors, "notion") ? 1 : 0

  name = "notion-connector"

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

resource "aws_service_discovery_service" "fireflies_connector" {
  count = contains(var.enabled_connectors, "fireflies") ? 1 : 0

  name = "fireflies-connector"

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

resource "aws_service_discovery_service" "imap_connector" {
  count = contains(var.enabled_connectors, "imap") ? 1 : 0

  name = "imap-connector"

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

resource "aws_service_discovery_service" "linear_connector" {
  count = contains(var.enabled_connectors, "linear") ? 1 : 0

  name = "linear-connector"

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

resource "aws_service_discovery_service" "clickup_connector" {
  count = contains(var.enabled_connectors, "clickup") ? 1 : 0

  name = "clickup-connector"

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

resource "aws_service_discovery_service" "filesystem_connector" {
  count = contains(var.enabled_connectors, "filesystem") ? 1 : 0

  name = "filesystem-connector"

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

resource "aws_service_discovery_service" "nextcloud_connector" {
  count = contains(var.enabled_connectors, "nextcloud") ? 1 : 0

  name = "nextcloud-connector"

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
