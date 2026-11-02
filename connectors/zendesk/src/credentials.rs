use std::fmt;

use anyhow::{Result, bail};
use serde::Deserialize;

#[derive(Clone, Deserialize)]
#[serde(untagged)]
pub enum ZendeskCredentials {
    ApiToken { email: String, api_token: String },
    Bearer { access_token: String },
}

impl ZendeskCredentials {
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::ApiToken { email, api_token } => {
                require_non_blank("email", email)?;
                require_non_blank("api_token", api_token)
            }
            Self::Bearer { access_token } => require_non_blank("access_token", access_token),
        }
    }
}

fn require_non_blank(field: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        bail!("Zendesk credential `{field}` must not be empty");
    }
    Ok(())
}

impl fmt::Debug for ZendeskCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ApiToken { email, .. } => f
                .debug_struct("ApiToken")
                .field("email", email)
                .field("api_token", &"<redacted>")
                .finish(),
            Self::Bearer { .. } => f
                .debug_struct("Bearer")
                .field("access_token", &"<redacted>")
                .finish(),
        }
    }
}
