use anyhow::{Result, bail};
use serde::{Deserialize, Deserializer};

/// Per-source Zendesk configuration stored in `Source.config`.
#[derive(Debug, Clone, Deserialize)]
pub struct ZendeskConfig {
    #[serde(deserialize_with = "deserialize_subdomain")]
    pub subdomain: String,
    #[serde(default = "default_true")]
    pub sync_tickets: bool,
    #[serde(default = "default_true")]
    pub sync_help_center: bool,
}

fn default_true() -> bool {
    true
}

fn deserialize_subdomain<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    let raw = String::deserialize(deserializer)?;
    validate_subdomain(&raw).map_err(serde::de::Error::custom)
}

/// One DNS label, so the subdomain cannot change the host the credential is sent to.
pub fn validate_subdomain(raw: &str) -> Result<String> {
    let value = raw.trim();
    let alnum = |c: char| c.is_ascii_alphanumeric();
    let valid = (1..=63).contains(&value.len())
        && value.starts_with(alnum)
        && value.ends_with(alnum)
        && value.chars().all(|c| alnum(c) || c == '-');
    if !valid {
        bail!(
            "'{value}' is not a valid Zendesk subdomain: for https://acme.zendesk.com use 'acme' (letters, digits and hyphens only)"
        );
    }
    Ok(value.to_string())
}

impl ZendeskConfig {
    pub fn from_source_config(config: &serde_json::Value) -> Result<Self> {
        Ok(serde_json::from_value(config.clone())?)
    }

    pub fn api_base_url(&self) -> String {
        format!("https://{}.zendesk.com/api/v2", self.subdomain)
    }

    pub fn agent_ticket_url(&self, ticket_id: u64) -> String {
        format!(
            "https://{}.zendesk.com/agent/tickets/{ticket_id}",
            self.subdomain
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn subdomain_accepts_one_dns_label() {
        for ok in ["example", "ex-1", "A1", " example "] {
            assert_eq!(validate_subdomain(ok).unwrap(), ok.trim());
        }
        assert!(validate_subdomain(&"a".repeat(63)).is_ok());
    }

    #[test]
    fn subdomain_rejects_anything_else() {
        let long = "a".repeat(64);
        for bad in ["a/x#", "a.b", "a@b", "a b", "-a", "a-", "", &long] {
            assert!(validate_subdomain(bad).is_err(), "{bad:?} was accepted");
        }
    }

    #[test]
    fn config_defaults_and_validation() {
        let config = ZendeskConfig::from_source_config(&json!({"subdomain": "example"})).unwrap();
        assert!(config.sync_tickets && config.sync_help_center);
        assert!(ZendeskConfig::from_source_config(&json!({"subdomain": "evil.com/x"})).is_err());
        assert!(ZendeskConfig::from_source_config(&json!({})).is_err());
    }
}
