use redis::AsyncCommands;
use regex::Regex;
use shared::models::SearchOperator;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{error, info, warn};

#[derive(Debug, Clone)]
pub struct OperatorMapping {
    pub attribute_key: String,
    pub value_type: String, // "person", "text", "datetime"
}

struct RegistryInner {
    operators: HashMap<String, OperatorMapping>,
    operator_regex: Option<Regex>,
}

#[derive(Clone)]
pub struct OperatorRegistry {
    data: Arc<RwLock<RegistryInner>>,
    redis_client: Option<redis::Client>,
}

impl OperatorRegistry {
    pub fn new(redis_client: redis::Client) -> Self {
        Self {
            data: Arc::new(RwLock::new(RegistryInner {
                operators: HashMap::new(),
                operator_regex: None,
            })),
            redis_client: Some(redis_client),
        }
    }

    #[cfg(test)]
    pub fn with_operators(operators: Vec<SearchOperator>) -> Self {
        let map = dedup_operators(operators);
        let regex = build_operator_regex(map.keys().map(|s| s.as_str()));

        Self {
            data: Arc::new(RwLock::new(RegistryInner {
                operators: map,
                operator_regex: Some(regex),
            })),
            redis_client: None,
        }
    }

    pub async fn refresh(&self) -> anyhow::Result<()> {
        let redis_client = self
            .redis_client
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No Redis client configured"))?;

        let mut conn = redis_client.get_multiplexed_async_connection().await?;
        let json: Option<String> = conn.get("search:operators").await?;

        let operators: Vec<SearchOperator> = match json {
            Some(j) => serde_json::from_str(&j)?,
            None => Vec::new(),
        };

        let map = dedup_operators(operators);
        let regex = build_operator_regex(map.keys().map(|s| s.as_str()));

        let count = map.len();
        let mut data = self.data.write().await;
        data.operators = map;
        data.operator_regex = Some(regex);

        info!(
            "Operator registry refreshed with {} distinct operators",
            count
        );
        Ok(())
    }

    pub async fn get(&self, operator: &str) -> Option<OperatorMapping> {
        let data = self.data.read().await;
        data.operators.get(operator).cloned()
    }

    pub async fn operator_regex(&self) -> Option<Regex> {
        let data = self.data.read().await;
        data.operator_regex.clone()
    }

    pub fn start_background_refresh(self: &Arc<Self>, interval_secs: u64) {
        let registry = Arc::clone(self);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(interval_secs));
            loop {
                interval.tick().await;
                if let Err(e) = registry.refresh().await {
                    error!("Failed to refresh operator registry: {}", e);
                }
            }
        });
    }
}

fn dedup_operators(operators: Vec<SearchOperator>) -> HashMap<String, OperatorMapping> {
    let mut map: HashMap<String, OperatorMapping> = HashMap::new();
    for op in operators {
        if let Some(existing) = map.get(&op.operator) {
            if existing.attribute_key != op.attribute_key {
                warn!(
                    "Duplicate operator '{}': keeping '{}', ignoring '{}'",
                    op.operator, existing.attribute_key, op.attribute_key
                );
            }
        } else {
            map.insert(
                op.operator,
                OperatorMapping {
                    attribute_key: op.attribute_key,
                    value_type: op.value_type,
                },
            );
        }
    }
    map
}

fn build_operator_regex<'a>(operator_names: impl Iterator<Item = &'a str>) -> Regex {
    let names: Vec<&str> = operator_names.collect();
    if names.is_empty() {
        return Regex::new(r#"(?i)\b(by|in|type|before|after):("([^"]+)"|(\S+))"#).unwrap();
    }

    let dynamic = names
        .iter()
        .map(|n| regex::escape(n))
        .collect::<Vec<_>>()
        .join("|");

    let pattern = format!(
        r#"(?i)\b(by|in|type|before|after|{}):(\"([^\"]+)\"|(\S+))"#,
        dynamic
    );

    Regex::new(&pattern).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_with_operators() {
        let registry = OperatorRegistry::with_operators(vec![
            SearchOperator {
                operator: "from".to_string(),
                attribute_key: "sender".to_string(),
                value_type: "person".to_string(),
            },
            SearchOperator {
                operator: "channel".to_string(),
                attribute_key: "channel_name".to_string(),
                value_type: "text".to_string(),
            },
        ]);

        let mapping = registry.get("from").await.unwrap();
        assert_eq!(mapping.attribute_key, "sender");
        assert_eq!(mapping.value_type, "person");

        let mapping = registry.get("channel").await.unwrap();
        assert_eq!(mapping.attribute_key, "channel_name");

        assert!(registry.get("unknown").await.is_none());
    }

    #[tokio::test]
    async fn test_duplicate_operator_first_wins() {
        let registry = OperatorRegistry::with_operators(vec![
            SearchOperator {
                operator: "status".to_string(),
                attribute_key: "status".to_string(),
                value_type: "text".to_string(),
            },
            SearchOperator {
                operator: "status".to_string(),
                attribute_key: "state".to_string(),
                value_type: "text".to_string(),
            },
        ]);

        let mapping = registry.get("status").await.unwrap();
        assert_eq!(mapping.attribute_key, "status");
    }

    #[tokio::test]
    async fn test_same_operator_same_key_no_conflict() {
        let registry = OperatorRegistry::with_operators(vec![
            SearchOperator {
                operator: "from".to_string(),
                attribute_key: "sender".to_string(),
                value_type: "person".to_string(),
            },
            SearchOperator {
                operator: "from".to_string(),
                attribute_key: "sender".to_string(),
                value_type: "person".to_string(),
            },
        ]);

        let mapping = registry.get("from").await.unwrap();
        assert_eq!(mapping.attribute_key, "sender");
    }

    #[tokio::test]
    async fn test_operator_regex_includes_dynamic_operators() {
        let registry = OperatorRegistry::with_operators(vec![SearchOperator {
            operator: "channel".to_string(),
            attribute_key: "channel_name".to_string(),
            value_type: "text".to_string(),
        }]);

        let regex = registry.operator_regex().await.unwrap();
        assert!(regex.is_match("channel:general"));
        assert!(regex.is_match("in:slack"));
        assert!(regex.is_match("type:pdf"));
    }
}
