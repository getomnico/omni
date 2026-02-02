use shared::DatabaseConfig;

#[derive(Debug, Clone)]
pub struct FileSystemConnectorConfig {
    pub database: DatabaseConfig,
}

impl FileSystemConnectorConfig {
    pub fn from_env() -> Self {
        let database = DatabaseConfig::from_env();

        Self { database }
    }
}
