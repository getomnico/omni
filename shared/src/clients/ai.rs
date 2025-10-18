use anyhow::{anyhow, Result};
use base64::{engine::general_purpose, Engine as _};
use futures_util::{Stream, StreamExt};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::pin::Pin;
use tracing::{debug, error};

use crate::telemetry::http_client::RequestBuilderExt;

#[derive(Serialize)]
pub struct EmbeddingRequest {
    pub texts: Vec<String>,
    pub task: Option<String>,
    pub chunk_size: Option<i32>,
    pub chunking_mode: Option<String>,
    pub priority: Option<String>, // "high", "normal", or "low"
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

#[derive(Serialize)]
pub struct PDFExtractionRequest {
    pub pdf_bytes: String, // Base64-encoded PDF bytes
}

#[derive(Deserialize)]
pub struct PDFExtractionResponse {
    pub text: String,
    pub page_count: i32,
    pub error: Option<String>,
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

    pub async fn generate_embeddings(&self, texts: Vec<String>) -> Result<Vec<TextEmbedding>> {
        self.generate_embeddings_with_options(
            texts,
            Some("retrieval.query".to_string()),
            None,
            Some("none".to_string()),
            None, // Default priority
        )
        .await
    }

    pub async fn generate_embeddings_with_options(
        &self,
        texts: Vec<String>,
        task: Option<String>,
        chunk_size: Option<i32>,
        chunking_mode: Option<String>,
        priority: Option<String>,
    ) -> Result<Vec<TextEmbedding>> {
        let request = EmbeddingRequest {
            texts,
            task,
            chunk_size,
            chunking_mode,
            priority,
        };

        let response = self
            .client
            .post(format!("{}/embeddings", self.base_url))
            .json(&request)
            .with_trace_context()
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
        let embeddings = self.generate_embeddings(vec![text.to_string()]).await?;
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
            .with_trace_context()
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(anyhow::anyhow!(
                "AI service returned error status: {}",
                response.status()
            ));
        }

        // Get the actual streaming bytes from the response
        let byte_stream = response.bytes_stream();

        // Convert bytes stream to string stream with proper UTF-8 handling
        let string_stream = {
            let mut buffer = Vec::new();
            byte_stream.map(move |chunk_result| {
                match chunk_result {
                    Ok(chunk) => {
                        // Add new bytes to buffer
                        buffer.extend_from_slice(&chunk);

                        // Try to convert buffer to UTF-8 string
                        match std::str::from_utf8(&buffer) {
                            Ok(valid_str) => {
                                // All bytes form valid UTF-8, return the string and clear buffer
                                let result = valid_str.to_string();
                                buffer.clear();
                                Ok(result)
                            }
                            Err(error) => {
                                // Check if error is due to incomplete UTF-8 sequence at the end
                                let valid_up_to = error.valid_up_to();
                                if valid_up_to > 0 {
                                    // Extract valid UTF-8 portion
                                    let valid_str = std::str::from_utf8(&buffer[..valid_up_to])
                                        .expect("Should be valid UTF-8");
                                    let result = valid_str.to_string();

                                    // Keep incomplete bytes in buffer for next chunk
                                    buffer.drain(..valid_up_to);
                                    Ok(result)
                                } else {
                                    // No valid UTF-8 at all, keep accumulating
                                    Ok(String::new())
                                }
                            }
                        }
                    }
                    Err(e) => Err(anyhow!("Stream error: {}", e)),
                }
            })
        };

        Ok(Box::pin(string_stream))
    }

    /// Extract text from a PDF file
    pub async fn extract_pdf_text(&self, pdf_bytes: Vec<u8>) -> Result<PDFExtractionResponse> {
        debug!(
            "Sending PDF ({} bytes) to AI service for text extraction",
            pdf_bytes.len()
        );

        // Encode PDF bytes as base64
        let pdf_bytes_b64 = general_purpose::STANDARD.encode(&pdf_bytes);
        let request = PDFExtractionRequest {
            pdf_bytes: pdf_bytes_b64,
        };

        let response = self
            .client
            .post(format!("{}/extract_pdf", self.base_url))
            .json(&request)
            .with_trace_context()
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await?;
            error!(
                "PDF extraction failed with status {}: {}",
                status, error_text
            );
            return Err(anyhow!(
                "PDF extraction failed with status {}: {}",
                status,
                error_text
            ));
        }

        let extraction_response: PDFExtractionResponse = response.json().await?;

        if let Some(ref error) = extraction_response.error {
            debug!("PDF extraction completed with error: {}", error);
        } else {
            debug!(
                "PDF extraction successful: {} pages, {} characters",
                extraction_response.page_count,
                extraction_response.text.len()
            );
        }

        Ok(extraction_response)
    }
}
