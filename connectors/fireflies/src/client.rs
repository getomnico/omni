use anyhow::{Context, Result, anyhow};
use reqwest::Client;
use serde_json::json;
use tracing::{debug, warn};

use crate::config::{BATCH_SIZE, FIREFLIES_GRAPHQL_URL, TRANSCRIPTS_QUERY};
use crate::models::{GraphQLResponse, Transcript};

pub struct FirefliesClient {
    client: Client,
}

impl FirefliesClient {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
        }
    }

    pub async fn query_transcripts(
        &self,
        api_key: &str,
        limit: i32,
        skip: i32,
        from_date: Option<&str>,
    ) -> Result<Vec<Transcript>> {
        let mut variables = json!({
            "limit": limit,
            "skip": skip,
        });

        if let Some(date) = from_date {
            variables["fromDate"] = json!(date);
        }

        let body = json!({
            "query": TRANSCRIPTS_QUERY,
            "variables": variables,
        });

        let response = self
            .client
            .post(FIREFLIES_GRAPHQL_URL)
            .header("Authorization", format!("Bearer {}", api_key))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .context("Failed to send GraphQL request to Fireflies")?;

        let status = response.status();

        if status.as_u16() == 401 || status.as_u16() == 403 {
            return Err(anyhow!(
                "Authentication failed ({}). Check your Fireflies API key.",
                status
            ));
        }

        if status.as_u16() == 429 {
            return Err(anyhow!("Rate limited by Fireflies API. Try again later."));
        }

        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(anyhow!("Fireflies API returned HTTP {}: {}", status, body));
        }

        let response_text = response
            .text()
            .await
            .context("Failed to read Fireflies response body")?;
        debug!("Fireflies API response: {}", response_text);

        let gql_response: GraphQLResponse = match serde_json::from_str(&response_text) {
            Ok(r) => r,
            Err(e) => {
                warn!("Failed to parse Fireflies GraphQL response: {}", e);
                return Err(e).context("Failed to parse Fireflies GraphQL response");
            }
        };

        if let Some(errors) = &gql_response.errors {
            if !errors.is_empty() {
                let messages: Vec<&str> = errors.iter().map(|e| e.message.as_str()).collect();
                return Err(anyhow!("GraphQL errors: {}", messages.join("; ")));
            }
        }

        Ok(gql_response.data.map(|d| d.transcripts).unwrap_or_default())
    }

    pub async fn test_connection(&self, api_key: &str) -> Result<()> {
        debug!("Testing Fireflies API connection...");
        let transcripts = self.query_transcripts(api_key, 1, 0, None).await?;
        debug!(
            "Fireflies connection test successful, got {} transcript(s)",
            transcripts.len()
        );
        Ok(())
    }

    pub async fn fetch_all_transcripts(
        &self,
        api_key: &str,
        from_date: Option<&str>,
    ) -> Result<Vec<Transcript>> {
        let mut all_transcripts = Vec::new();
        let mut skip = 0;

        loop {
            debug!(
                "Fetching transcripts batch: skip={}, limit={}",
                skip, BATCH_SIZE
            );

            let batch = match self
                .query_transcripts(api_key, BATCH_SIZE, skip, from_date)
                .await
            {
                Ok(b) => b,
                Err(e) => {
                    if skip > 0 {
                        warn!(
                            "Failed to fetch batch at skip={}, returning {} transcripts collected so far: {}",
                            skip,
                            all_transcripts.len(),
                            e
                        );
                        break;
                    }
                    return Err(e);
                }
            };

            let batch_size = batch.len();
            debug!("Received {} transcripts in batch", batch_size);
            all_transcripts.extend(batch);

            if (batch_size as i32) < BATCH_SIZE {
                break;
            }

            skip += BATCH_SIZE;
        }

        Ok(all_transcripts)
    }
}
