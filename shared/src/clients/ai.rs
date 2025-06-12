use anyhow::Result;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::warn;

#[derive(Serialize)]
pub struct EmbeddingRequest {
    pub text: String,
    pub model: String,
}

#[derive(Deserialize)]
pub struct EmbeddingResponse {
    pub embedding: Vec<f32>,
    #[allow(dead_code)]
    pub model: String,
    pub dimensions: usize,
}

#[derive(Clone)]
pub struct AIClient {
    client: Client,
    base_url: String,
}

impl AIClient {
    pub fn new(ai_service_url: String) -> Self {
        Self {
            client: Client::new(),
            base_url: ai_service_url,
        }
    }

    pub async fn generate_embedding(&self, text: &str) -> Result<Vec<f32>> {
        let request = EmbeddingRequest {
            text: text.to_string(),
            model: "e5-large-v2".to_string(),
        };

        let response = self
            .client
            .post(format!("{}/embeddings", self.base_url))
            .json(&request)
            .send()
            .await;

        match response {
            Ok(res) => {
                if res.status().is_success() {
                    let embedding_response: EmbeddingResponse = res.json().await?;
                    if embedding_response.dimensions != 1024 {
                        warn!(
                            "Unexpected embedding dimensions: {} (expected 1024)",
                            embedding_response.dimensions
                        );
                    }
                    Ok(embedding_response.embedding)
                } else {
                    warn!(
                        "AI service returned error status: {}, using placeholder embedding",
                        res.status()
                    );
                    Ok(vec![0.0; 1024])
                }
            }
            Err(e) => {
                warn!(
                    "Failed to connect to AI service: {}, using placeholder embedding",
                    e
                );
                Ok(vec![0.0; 1024])
            }
        }
    }
}