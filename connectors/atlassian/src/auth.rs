use anyhow::{anyhow, Result};
use base64::Engine;
use chrono::Utc;
use omni_connector_sdk::SourceType;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AtlassianCredentials {
    pub base_url: String,
    pub user_email: String,
    pub api_token: String,
    pub validated_at: i64,
}

impl AtlassianCredentials {
    pub fn new(base_url: String, user_email: String, api_token: String) -> Self {
        Self {
            base_url,
            user_email,
            api_token,
            validated_at: Utc::now().timestamp_millis(),
        }
    }

    pub fn is_valid(&self) -> bool {
        // API tokens don't expire, but we'll consider them stale after 24 hours
        // for re-validation purposes
        let now = Utc::now().timestamp_millis();
        let one_day_ms = 24 * 60 * 60 * 1000;
        (now - self.validated_at) < one_day_ms
    }

    pub fn get_basic_auth_header(&self) -> String {
        let auth_string = format!("{}:{}", self.user_email, self.api_token);
        let encoded = base64::engine::general_purpose::STANDARD.encode(auth_string.as_bytes());
        format!("Basic {}", encoded)
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AtlassianUserResponse {
    #[serde(rename = "accountId")]
    pub account_id: String,
    #[serde(rename = "displayName")]
    pub display_name: String,
    #[serde(rename = "emailAddress")]
    pub email_address: String,
    pub active: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ConfluenceUserResponse {
    pub results: Vec<ConfluenceUserInfo>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ConfluenceUserInfo {
    #[serde(rename = "type")]
    pub user_type: String,
    #[serde(rename = "accountId")]
    pub account_id: String,
    #[serde(rename = "displayName")]
    pub display_name: String,
    pub email: Option<String>,
}

pub struct AuthManager {
    client: Client,
}

impl AuthManager {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
        }
    }

    pub async fn validate_credentials(
        &self,
        base_url: &str,
        user_email: &str,
        api_token: &str,
        source_type: Option<&SourceType>,
    ) -> Result<AtlassianCredentials> {
        info!("Validating Atlassian credentials for user: {}", user_email);

        let auth_header = AtlassianCredentials::new(
            base_url.to_string(),
            user_email.to_string(),
            api_token.to_string(),
        )
        .get_basic_auth_header();

        let validate_jira = source_type != Some(&SourceType::Confluence);
        let validate_confluence = source_type != Some(&SourceType::Jira);

        // Test JIRA API access
        let mut jira_account_id = None;
        if validate_jira {
            let jira_url = format!("{}/rest/api/3/myself", base_url);
            let jira_response = self
                .client
                .get(&jira_url)
                .header("Authorization", &auth_header)
                .header("Accept", "application/json")
                .header("Content-Type", "application/json")
                .send()
                .await?;

            if !jira_response.status().is_success() {
                let status = jira_response.status();
                let error_text = jira_response.text().await?;
                return Err(anyhow!(
                    "Failed to validate JIRA credentials: HTTP {} - {}",
                    status,
                    error_text
                ));
            }

            let jira_user: AtlassianUserResponse = jira_response.json().await?;
            debug!(
                "JIRA validation successful for user: {}",
                jira_user.display_name
            );

            if !jira_user.active {
                return Err(anyhow!("User account is not active"));
            }

            if jira_user.email_address != user_email {
                return Err(anyhow!(
                    "Email mismatch: expected {}, got {}",
                    user_email,
                    jira_user.email_address
                ));
            }

            info!(
                "Successfully validated credentials for user: {} (Account ID: {})",
                jira_user.display_name, jira_user.account_id
            );
            jira_account_id = Some(jira_user.account_id);
        }

        // Test Confluence API access
        if validate_confluence {
            let confluence_url = format!("{}/wiki/rest/api/user/current", base_url);
            let confluence_response = self
                .client
                .get(&confluence_url)
                .header("Authorization", &auth_header)
                .header("Accept", "application/json")
                .header("Content-Type", "application/json")
                .send()
                .await?;

            if !confluence_response.status().is_success() {
                let status = confluence_response.status();
                let error_text = confluence_response.text().await?;
                return Err(anyhow!(
                    "Failed to validate Confluence credentials: HTTP {} - {}",
                    status,
                    error_text
                ));
            }

            let confluence_user: ConfluenceUserInfo = confluence_response.json().await?;
            debug!(
                "Confluence validation successful for user: {}",
                confluence_user.display_name
            );

            if let Some(jira_id) = &jira_account_id {
                if confluence_user.account_id != *jira_id {
                    return Err(anyhow!(
                        "Account ID mismatch between JIRA and Confluence for user {}",
                        user_email
                    ));
                }
            }

            info!(
                "Successfully validated Confluence credentials for user: {}",
                confluence_user.display_name
            );
        }

        Ok(AtlassianCredentials::new(
            base_url.to_string(),
            user_email.to_string(),
            api_token.to_string(),
        ))
    }

    pub async fn ensure_valid_credentials(
        &self,
        creds: &mut AtlassianCredentials,
        source_type: Option<&SourceType>,
    ) -> Result<()> {
        if !creds.is_valid() {
            debug!("Re-validating API token");
            let new_creds = self
                .validate_credentials(
                    &creds.base_url,
                    &creds.user_email,
                    &creds.api_token,
                    source_type,
                )
                .await?;
            *creds = new_creds;
        }
        Ok(())
    }

    pub async fn test_jira_permissions(&self, creds: &AtlassianCredentials) -> Result<Vec<String>> {
        let auth_header = creds.get_basic_auth_header();
        let url = format!("{}/rest/api/3/project", creds.base_url);

        let response = self
            .client
            .get(&url)
            .header("Authorization", &auth_header)
            .header("Accept", "application/json")
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await?;
            return Err(anyhow!(
                "Failed to fetch JIRA projects: HTTP {} - {}",
                status,
                error_text
            ));
        }

        let projects: Vec<serde_json::Value> = response.json().await?;
        let project_keys: Vec<String> = projects
            .iter()
            .filter_map(|p| p.get("key").and_then(|k| k.as_str().map(String::from)))
            .collect();

        debug!("Found {} accessible JIRA projects", project_keys.len());
        Ok(project_keys)
    }

    pub async fn test_confluence_permissions(
        &self,
        creds: &AtlassianCredentials,
    ) -> Result<Vec<String>> {
        let auth_header = creds.get_basic_auth_header();
        let url = format!("{}/wiki/rest/api/space?limit=100", creds.base_url);

        let response = self
            .client
            .get(&url)
            .header("Authorization", &auth_header)
            .header("Accept", "application/json")
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await?;
            return Err(anyhow!(
                "Failed to fetch Confluence spaces: HTTP {} - {}",
                status,
                error_text
            ));
        }

        let response_data: serde_json::Value = response.json().await?;
        let empty_vec = vec![];
        let spaces = response_data
            .get("results")
            .and_then(|r| r.as_array())
            .unwrap_or(&empty_vec);

        let space_keys: Vec<String> = spaces
            .iter()
            .filter_map(|s| s.get("key").and_then(|k| k.as_str().map(String::from)))
            .collect();

        debug!("Found {} accessible Confluence spaces", space_keys.len());
        Ok(space_keys)
    }
}
