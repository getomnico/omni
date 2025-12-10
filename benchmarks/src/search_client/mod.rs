use anyhow::Result;
use omni_searcher::models::{SearchMode, SearchRequest, SearchResponse};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use shared::SourceType;
use std::time::Duration;
use tracing::{debug, error};

pub struct OmniSearchClient {
    client: Client,
    base_url: String,
}

impl OmniSearchClient {
    pub fn new(base_url: &str) -> Result<Self> {
        let client = Client::builder().timeout(Duration::from_secs(30)).build()?;

        Ok(Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
        })
    }

    pub async fn search(&self, request: &SearchRequest) -> Result<SearchResponse> {
        let url = format!("{}/search", self.base_url);

        debug!("Sending search request to: {}", url);
        debug!("Request: {:?}", request);

        let response = self.client.post(&url).json(request).send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            error!(
                "Search request failed with status {}: {}",
                status, error_text
            );
            return Err(anyhow::anyhow!(
                "Search request failed with status {}: {}",
                status,
                error_text
            ));
        }

        let search_response: SearchResponse = response.json().await?;
        debug!("Received {} results", search_response.results.len());

        Ok(search_response)
    }

    pub async fn health_check(&self) -> Result<bool> {
        let url = format!("{}/health", self.base_url);

        match self.client.get(&url).send().await {
            Ok(response) => Ok(response.status().is_success()),
            Err(_) => Ok(false),
        }
    }

    pub async fn get_index_stats(&self) -> Result<IndexStats> {
        let url = format!("{}/stats", self.base_url);

        let response = self.client.get(&url).send().await?;

        if !response.status().is_success() {
            return Err(anyhow::anyhow!("Failed to get index stats"));
        }

        let stats: IndexStats = response.json().await?;
        Ok(stats)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexStats {
    pub total_documents: i64,
    pub total_sources: i64,
    pub index_size_mb: f64,
    pub last_updated: String,
}

pub fn create_search_request(query: String, search_mode: SearchMode) -> SearchRequest {
    SearchRequest {
        query,
        mode: Some(search_mode),
        limit: Some(20),
        offset: Some(0),
        source_types: None,
        content_types: None,
        include_facets: Some(false),
        user_email: None,
        user_id: None,
        is_generated_query: None,
        original_user_query: None,
        document_id: None,
        document_content_start_line: None,
        document_content_end_line: None,
        ignore_typos: None,
    }
}

pub fn with_limit(mut request: SearchRequest, limit: i64) -> SearchRequest {
    request.limit = Some(limit);
    request
}

pub fn with_offset(mut request: SearchRequest, offset: i64) -> SearchRequest {
    request.offset = Some(offset);
    request
}

pub fn with_sources(mut request: SearchRequest, sources: Vec<SourceType>) -> SearchRequest {
    request.source_types = Some(sources);
    request
}

pub fn with_content_types(mut request: SearchRequest, content_types: Vec<String>) -> SearchRequest {
    request.content_types = Some(content_types);
    request
}

pub fn with_facets(mut request: SearchRequest, include_facets: bool) -> SearchRequest {
    request.include_facets = Some(include_facets);
    request
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_search_request_builder() {
        let request = create_search_request("test query".to_string(), SearchMode::Hybrid);
        let request = with_limit(request, 10);
        let request = with_offset(request, 0);
        let request = with_sources(request, vec!["google".to_string()]);
        let request = with_facets(request, true);

        assert_eq!(request.query, "test query");
        assert_eq!(request.mode, Some(SearchMode::Hybrid));
        assert_eq!(request.limit, Some(10));
        assert_eq!(request.offset, Some(0));
        assert_eq!(request.source_types, Some(vec!["google".to_string()]));
        assert_eq!(request.include_facets, Some(true));
    }
}
