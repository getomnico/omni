use anyhow::{anyhow, Result};
use futures_util::Stream;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::pin::Pin;
use tracing::error;

#[derive(Serialize)]
pub struct EmbeddingRequest {
    pub texts: Vec<String>,
    pub task: Option<String>,
    pub chunk_size: Option<i32>,
    pub chunking_mode: Option<String>,
}

#[derive(Deserialize)]
pub struct EmbeddingResponse {
    pub embeddings: Vec<Vec<Vec<f32>>>, // embeddings per text per chunk
    pub chunks_count: Vec<i32>,         // number of chunks per text
    pub chunks: Vec<Vec<(i32, i32)>>,   // character offset spans for each chunk
    pub model_name: String,             // name of the model used for embeddings
}

#[derive(Debug, Clone)]
pub struct TextEmbedding {
    pub chunk_embeddings: Vec<Vec<f32>>,
    pub chunk_spans: Vec<(i32, i32)>, // character start/end offsets
    pub model_name: Option<String>,   // name of the model used for embeddings
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

    pub async fn generate_embeddings(&self, texts: &[String]) -> Result<Vec<TextEmbedding>> {
        self.generate_embeddings_with_options(
            texts,
            Some("retrieval.query".to_string()),
            None,
            Some("none".to_string()),
        )
        .await
    }

    pub async fn generate_embeddings_with_options(
        &self,
        texts: &[String],
        task: Option<String>,
        chunk_size: Option<i32>,
        chunking_mode: Option<String>,
    ) -> Result<Vec<TextEmbedding>> {
        let request = EmbeddingRequest {
            texts: texts.to_vec(),
            task,
            chunk_size,
            chunking_mode,
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

                    let mut result = Vec::new();
                    for (i, text_embeddings) in embedding_response.embeddings.iter().enumerate() {
                        let chunk_spans = embedding_response
                            .chunks
                            .get(i)
                            .cloned()
                            .unwrap_or_default();
                        result.push(TextEmbedding {
                            chunk_embeddings: text_embeddings.clone(),
                            chunk_spans,
                            model_name: Some(embedding_response.model_name.clone()),
                        });
                    }

                    Ok(result)
                } else {
                    error!(
                        "AI service returned error status: {}, embeddings gen failed.",
                        res.status()
                    );
                    let status_code = res.status();
                    let resp_text = res.text().await?;
                    Err(anyhow!(
                        "Embeddings API failed with error: [{}] {:?}",
                        status_code,
                        resp_text
                    ))
                }
            }
            Err(e) => {
                error!("Failed to connect to embeddings API: {:?}", e);
                Err(anyhow!("Failed to connect to embeddings API: {:?}.", e))
            }
        }
    }

    // Keep backward compatibility method for single text
    #[deprecated(note = "Use generate_embeddings instead")]
    pub async fn generate_embedding(&self, text: &str) -> Result<Vec<f32>> {
        let embeddings = self.generate_embeddings(&[text.to_string()]).await?;
        if let Some(first_text) = embeddings.first() {
            if let Some(first_chunk) = first_text.chunk_embeddings.first() {
                return Ok(first_chunk.clone());
            }
        }
        Ok(vec![0.0; 1024])
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
