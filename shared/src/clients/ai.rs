use anyhow::Result;
use futures_util::Stream;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::pin::Pin;
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

#[derive(Serialize)]
pub struct PromptRequest {
    pub prompt: String,
    pub max_tokens: Option<i32>,
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    pub stream: Option<bool>,
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

    /// Stream AI response from the prompt endpoint
    pub async fn stream_prompt(
        &self,
        prompt: &str,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<String>> + Send>>> {
        let request = PromptRequest {
            prompt: prompt.to_string(),
            max_tokens: Some(512),
            temperature: Some(0.7),
            top_p: Some(0.9),
            stream: Some(true),
        };

        let response = self
            .client
            .post(format!("{}/prompt", self.base_url))
            .json(&request)
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(anyhow::anyhow!(
                "AI service returned error status: {}",
                response.status()
            ));
        }

        // Convert response text to stream by reading it all at once
        // This is a simplified approach - in a real implementation you'd want proper streaming
        let text = response.text().await?;

        // Create a simple stream that yields the text in small chunks to simulate streaming
        let string_stream = futures_util::stream::iter(
            text.chars()
                .collect::<Vec<char>>()
                .chunks(5) // Send 5 characters at a time to simulate streaming
                .map(|chunk| Ok(chunk.iter().collect::<String>()))
                .collect::<Vec<_>>(),
        );

        Ok(Box::pin(string_stream))
    }
}
