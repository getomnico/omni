//! HTTP client for the Docling document conversion service.
//!
//! The Docling service converts documents to Markdown using AI-based extraction.
//! It provides superior PDF extraction, OCR support, and structure-aware output.

use anyhow::{anyhow, Context, Result};
use reqwest::{multipart, Client};
use serde::Deserialize;
use std::time::Duration;
use tracing::{debug, error};

/// Default timeout for polling a conversion job, which can take a very long time with Docling
const DEFAULT_TIMEOUT: Duration = Duration::from_mins(120);

/// Interval between status polls
const POLL_INTERVAL: Duration = Duration::from_secs(5);

/// Status of a conversion job
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum JobStatus {
    Pending,
    Running,
    Completed,
    Failed,
}

/// Response from the `/convert` endpoint
#[derive(Debug, Deserialize)]
struct SubmitResponse {
    job_id: String,
}

/// Response from the `/jobs/{job_id}` endpoint
#[derive(Debug, Deserialize)]
struct JobResponse {
    status: JobStatus,
    markdown: Option<String>,
    detail: Option<String>,
}

/// Response from the `/health` endpoint
#[derive(Debug, Deserialize)]
struct HealthResponse {
    status: String,
}

/// Client for the Docling document conversion service.
#[derive(Clone)]
pub struct DoclingClient {
    client: Client,
    base_url: String,
    timeout: Duration,
}

impl DoclingClient {
    /// Create a new Docling client with the given base URL.
    pub fn new(base_url: impl Into<String>) -> Self {
            let url = base_url.into();
        Self {
            client: Client::new(),
            base_url: url.trim_end_matches('/').to_string(),
            timeout: DEFAULT_TIMEOUT,
        }
    }

    /// Create a new Docling client from the DOCLING_URL environment variable.
    /// Returns None if the variable is not set.
    pub fn from_env() -> Option<Self> {
        std::env::var("DOCLING_URL").ok().map(Self::new)
    }

    /// Set the timeout for waiting on conversion jobs.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Check if the Docling service is healthy and ready.
    pub async fn health_check(&self) -> Result<bool> {
        let response = self
            .client
            .get(format!("{}/health", self.base_url))
            .send()
            .await
            .context("Failed to connect to Docling service")?;

        if response.status().is_success() {
            let health: HealthResponse = response.json().await?;
            Ok(health.status == "ok")
        } else if response.status().as_u16() == 503 {
            // Service is starting up
            Ok(false)
        } else {
            Err(anyhow!(
                "Docling health check failed with status: {}",
                response.status()
            ))
        }
    }

    /// Convert a document to Markdown.
    ///
    /// This is a blocking call that submits the document, polls for completion,
    /// and returns the resulting Markdown text.
    ///
    /// # Arguments
    /// * `data` - The raw document bytes
    /// * `filename` - The filename (used for format detection via extension)
    ///
    /// # Returns
    /// The extracted Markdown text, or an error if conversion fails.
    pub async fn convert(&self, data: &[u8], filename: &str) -> Result<String> {
        // Submit the conversion job
        let job_id = self.submit(data, filename).await?;
        debug!("Docling conversion job submitted: {}", job_id);

        // Poll for completion
        let start = std::time::Instant::now();
        loop {
            if start.elapsed() > self.timeout {
                return Err(anyhow!(
                    "Docling conversion timed out after {:?}",
                    self.timeout
                ));
            }

            let job = self.get_job(&job_id).await?;

            match job.status {
                JobStatus::Completed => {
                    let markdown = job.markdown.ok_or_else(|| {
                        anyhow!("Docling returned completed status but no markdown")
                    })?;
                    debug!(
                        "Docling conversion completed: {} chars in {:?}",
                        markdown.len(),
                        start.elapsed()
                    );
                    return Ok(markdown);
                }
                JobStatus::Failed => {
                    let detail = job.detail.unwrap_or_else(|| "Unknown error".to_string());
                    error!("Docling conversion failed: {}", detail);
                    return Err(anyhow!("Docling conversion failed: {}", detail));
                }
                JobStatus::Pending | JobStatus::Running => {
                    tokio::time::sleep(POLL_INTERVAL).await;
                }
            }
        }
    }

    /// Submit a document for conversion.
    /// Returns the job ID immediately.
    async fn submit(&self, data: &[u8], filename: &str) -> Result<String> {
        let part = multipart::Part::bytes(data.to_vec())
            .file_name(filename.to_string())
            .mime_str("application/octet-stream")?;

        let form = multipart::Form::new().part("file", part);

        let response = self
            .client
            .post(format!("{}/convert", self.base_url))
            .multipart(form)
            .send()
            .await
            .context("Failed to submit document to Docling")?;

        if response.status().as_u16() == 503 {
            return Err(anyhow!(
                "Docling service is starting up, models are being loaded. Try again shortly."
            ));
        }

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(anyhow!(
                "Docling conversion submission failed: {} - {}",
                status,
                body
            ));
        }

        let submit_response: SubmitResponse = response
            .json()
            .await
            .context("Failed to parse Docling response")?;

        Ok(submit_response.job_id)
    }

    /// Get the status of a conversion job.
    async fn get_job(&self, job_id: &str) -> Result<JobResponse> {
        let response = self
            .client
            .get(format!("{}/jobs/{}", self.base_url, job_id))
            .send()
            .await
            .context("Failed to poll Docling job")?;

        if response.status().as_u16() == 404 {
            return Err(anyhow!("Docling job not found: {}", job_id));
        }

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(anyhow!(
                "Failed to get Docling job status: {} - {}",
                status,
                body
            ));
        }

        let job: JobResponse = response
            .json()
            .await
            .context("Failed to parse Docling job response")?;

        Ok(job)
    }
}

/// Check if Docling conversion is enabled via environment variable.
///
/// Returns `Some(true)` or `Some(false)` if the env var is set to a non-empty value,
/// or `None` if unset or empty (meaning the caller should check Redis / UI setting).
pub fn docling_enabled_from_env() -> Option<bool> {
    match std::env::var("DOCLING_ENABLED") {
        Ok(v) if !v.is_empty() => Some(v.eq_ignore_ascii_case("true")),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_docling_enabled_from_env() {
        // Test with no env var set
        std::env::remove_var("DOCLING_ENABLED");
        assert_eq!(docling_enabled_from_env(), None);

        // Test with empty string (treated as unset)
        std::env::set_var("DOCLING_ENABLED", "");
        assert_eq!(docling_enabled_from_env(), None);

        // Test with false
        std::env::set_var("DOCLING_ENABLED", "false");
        assert_eq!(docling_enabled_from_env(), Some(false));

        // Test with true
        std::env::set_var("DOCLING_ENABLED", "true");
        assert_eq!(docling_enabled_from_env(), Some(true));

        // Test with TRUE (case insensitive)
        std::env::set_var("DOCLING_ENABLED", "TRUE");
        assert_eq!(docling_enabled_from_env(), Some(true));

        // Clean up
        std::env::remove_var("DOCLING_ENABLED");
    }

    #[test]
    fn test_new_trims_trailing_slash() {
        let client = DoclingClient::new("http://localhost:8003/");
        assert_eq!(client.base_url, "http://localhost:8003");

        let client = DoclingClient::new("http://localhost:8003///");
        assert_eq!(client.base_url, "http://localhost:8003");

        let client = DoclingClient::new("http://localhost:8003");
        assert_eq!(client.base_url, "http://localhost:8003");
    }
}
