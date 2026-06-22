use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "auth_type", rename_all = "snake_case")]
pub enum DarwinboxCredentials {
    Basic {
        username: String,
        password: String,
        api_key: String,
        dataset_key: String,
    },
    DynamicToken {
        client_id: String,
        client_secret: String,
        grant_type: String,
        code: Option<String>,
        refresh_token: Option<String>,
        api_key: Option<String>,
        dataset_key: String,
    },
    ClientCredentials {
        client_id: String,
        client_secret: String,
        api_key: Option<String>,
        dataset_key: String,
    },
}

impl DarwinboxCredentials {
    pub fn dataset_key(&self) -> &str {
        match self {
            Self::Basic { dataset_key, .. }
            | Self::DynamicToken { dataset_key, .. }
            | Self::ClientCredentials { dataset_key, .. } => dataset_key,
        }
    }

    pub fn api_key(&self) -> Option<&str> {
        match self {
            Self::Basic { api_key, .. } => Some(api_key),
            Self::DynamicToken { api_key, .. } | Self::ClientCredentials { api_key, .. } => {
                api_key.as_deref()
            }
        }
    }
}
