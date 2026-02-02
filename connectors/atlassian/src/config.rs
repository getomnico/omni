use shared::ConnectorConfig;

#[derive(Debug, Clone)]
pub struct AtlassianConnectorConfig {
    pub base: ConnectorConfig,
}

impl AtlassianConnectorConfig {
    pub fn from_env() -> Self {
        let base = ConnectorConfig::from_env();

        Self { base }
    }
}
