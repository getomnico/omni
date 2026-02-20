use anyhow::{anyhow, Result};
use chrono::Utc;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

use crate::models::AuthTestResponse;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SlackBotCredentials {
    pub bot_token: String,
    pub team_id: String,
    pub team_name: String,
    pub bot_user_id: String,
    pub validated_at: i64,
}

impl SlackBotCredentials {
    pub fn new(bot_token: String, auth_response: AuthTestResponse) -> Self {
        Self {
            bot_token,
            team_id: auth_response.team_id,
            team_name: auth_response.team,
            bot_user_id: auth_response.user_id,
            validated_at: Utc::now().timestamp_millis(),
        }
    }

    pub fn is_valid(&self) -> bool {
        // Bot tokens don't expire, but we'll consider them stale after 24 hours
        // for re-validation purposes
        let now = Utc::now().timestamp_millis();
        let one_day_ms = 24 * 60 * 60 * 1000;
        (now - self.validated_at) < one_day_ms
    }
}

const DEFAULT_SLACK_API_BASE: &str = "https://slack.com/api";

pub struct AuthManager {
    client: Client,
    base_url: String,
}

impl AuthManager {
    pub fn new() -> Self {
        Self::with_base_url(DEFAULT_SLACK_API_BASE.to_string())
    }

    pub fn with_base_url(base_url: String) -> Self {
        Self {
            client: Client::new(),
            base_url,
        }
    }

    pub async fn validate_bot_token(&self, bot_token: &str) -> Result<SlackBotCredentials> {
        info!("Validating Slack bot token");

        if !bot_token.starts_with("xoxb-") {
            return Err(anyhow!(
                "Invalid bot token format. Bot tokens should start with 'xoxb-'"
            ));
        }

        let url = format!("{}/auth.test", self.base_url);
        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", bot_token))
            .header("Content-Type", "application/json")
            .send()
            .await?;

        if !response.status().is_success() {
            let error_text = response.text().await?;
            return Err(anyhow!("Failed to validate bot token: {}", error_text));
        }

        let auth_response: AuthTestResponse = response.json().await?;

        if !auth_response.ok {
            return Err(anyhow!(
                "Token validation failed: {}",
                auth_response.error.unwrap_or("Unknown error".to_string())
            ));
        }

        debug!(
            "Bot token validated for team: {} ({})",
            auth_response.team, auth_response.team_id
        );

        Ok(SlackBotCredentials::new(
            bot_token.to_string(),
            auth_response,
        ))
    }

    pub async fn ensure_valid_credentials(&self, creds: &mut SlackBotCredentials) -> Result<()> {
        if !creds.is_valid() {
            debug!("Re-validating bot token");
            let new_creds = self.validate_bot_token(&creds.bot_token).await?;
            *creds = new_creds;
        }
        Ok(())
    }
}
