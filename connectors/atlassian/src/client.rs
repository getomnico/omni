use anyhow::{anyhow, Result};
use futures::stream::Stream;
use reqwest::{Client, Response, StatusCode};
use serde::de::DeserializeOwned;
use shared::rate_limiter::{RateLimiter, RetryableError};
use std::pin::Pin;
use std::time::Duration;
use tracing::{debug, warn};

use crate::auth::AtlassianCredentials;
use crate::models::{
    ConfluenceGetPagesResponse, ConfluenceGetSpacesResponse, ConfluencePage, ConfluenceSpace,
    JiraField, JiraIssue, JiraSearchResponse,
};

pub struct AtlassianClient {
    client: Client,
    rate_limiter: RateLimiter,
}

impl AtlassianClient {
    pub fn new() -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent("Omni/1.0 (Atlassian Connector)")
            .build()
            .expect("Failed to create HTTP client");

        // Atlassian API rate limits: ~10 requests per second for Cloud
        Self {
            client,
            rate_limiter: RateLimiter::new(10, 5),
        }
    }

    async fn make_request<T>(&self, request_fn: impl Fn() -> reqwest::RequestBuilder) -> Result<T>
    where
        T: DeserializeOwned,
    {
        self.rate_limiter
            .execute_with_retry(|| async {
                let request = request_fn();
                let response = request
                    .send()
                    .await
                    .map_err(|e| RetryableError::Transient(e.into()))?;

                match response.status() {
                    StatusCode::OK => {
                        let data: T = response
                            .json()
                            .await
                            .map_err(|e| RetryableError::Permanent(e.into()))?;
                        Ok(data)
                    }
                    StatusCode::TOO_MANY_REQUESTS => {
                        let retry_after = Self::extract_retry_after(&response);
                        Err(RetryableError::RateLimited {
                            retry_after,
                            message: "Atlassian API rate limit exceeded".to_string(),
                        })
                    }
                    StatusCode::UNAUTHORIZED => {
                        let error_text = response.text().await.unwrap_or_default();
                        Err(RetryableError::Permanent(anyhow!(
                            "Authentication failed: {}",
                            error_text
                        )))
                    }
                    StatusCode::FORBIDDEN => {
                        let error_text = response.text().await.unwrap_or_default();
                        Err(RetryableError::Permanent(anyhow!(
                            "Access forbidden: {}",
                            error_text
                        )))
                    }
                    StatusCode::NOT_FOUND => {
                        let error_text = response.text().await.unwrap_or_default();
                        Err(RetryableError::Permanent(anyhow!(
                            "Resource not found: {}",
                            error_text
                        )))
                    }
                    StatusCode::INTERNAL_SERVER_ERROR
                    | StatusCode::BAD_GATEWAY
                    | StatusCode::SERVICE_UNAVAILABLE
                    | StatusCode::GATEWAY_TIMEOUT => {
                        let status = response.status();
                        let error_text = response.text().await.unwrap_or_default();
                        Err(RetryableError::Transient(anyhow!(
                            "Server error: HTTP {} - {}",
                            status,
                            error_text
                        )))
                    }
                    _ => {
                        let status = response.status();
                        let error_text = response.text().await.unwrap_or_default();
                        Err(RetryableError::Permanent(anyhow!(
                            "Unexpected HTTP status {}: {}",
                            status,
                            error_text
                        )))
                    }
                }
            })
            .await
    }

    fn extract_retry_after(response: &Response) -> Duration {
        if let Some(retry_after) = response.headers().get("Retry-After") {
            if let Ok(retry_after_str) = retry_after.to_str() {
                if let Ok(seconds) = retry_after_str.parse::<u64>() {
                    return Duration::from_secs(seconds);
                }
            }
        }
        Duration::from_secs(60)
    }

    pub fn get_confluence_pages<'a>(
        &'a self,
        creds: &'a AtlassianCredentials,
        space_id: &'a str,
    ) -> Pin<Box<dyn Stream<Item = Result<ConfluencePage>> + Send + 'a>> {
        Box::pin(async_stream::stream! {
            let auth_header = creds.get_basic_auth_header();
            let mut url = format!("{}/wiki/api/v2/spaces/{}/pages", creds.base_url, space_id);
            let page_size = 250;
            let params = vec![
                ("limit", page_size.to_string()),
                ("body-format", "storage".to_string())
            ];

            loop {
                debug!("Fetching Confluence pages from space {}: {}, params: {:?}", space_id, url, params);

                let client = self.client.clone();
                let resp: Result<ConfluenceGetPagesResponse> = self
                    .make_request(|| {
                        client
                            .get(&url)
                            .query(&params)
                            .header("Authorization", &auth_header)
                            .header("Accept", "application/json")
                    })
                    .await;

                let resp = match resp {
                    Ok(r) => r,
                    Err(e) => {
                        yield Err(e);
                        return;
                    }
                };

                debug!("Fetched {} pages from Confluence space {}", resp.results.len(), space_id);

                for page in resp.results {
                    yield Ok(page);
                }

                debug!("Confluence get pages response links: {:?}", resp.links);
                if let Some(links) = resp.links {
                    if let Some(next) = links.next {
                        let base_url = links.base;
                        debug!("Next page available, base: {}, next: {:?}", base_url, next);
                        url = format!("{}{}", base_url, next);
                    } else {
                        debug!("All pages fetched.");
                        return;
                    }
                } else {
                    debug!("No links in response.");
                    return;
                }
            }
        })
    }

    pub async fn get_confluence_page_by_id(
        &self,
        creds: &AtlassianCredentials,
        page_id: &str,
        expand: &[&str],
    ) -> Result<ConfluencePage> {
        let auth_header = creds.get_basic_auth_header();
        let mut url = format!("{}/wiki/rest/api/content/{}", creds.base_url, page_id);

        if !expand.is_empty() {
            url.push_str(&format!("?expand={}", expand.join(",")));
        }

        debug!("Fetching Confluence page: {}", url);

        let client = self.client.clone();
        self.make_request(move || {
            client
                .get(&url)
                .header("Authorization", &auth_header)
                .header("Accept", "application/json")
        })
        .await
    }

    pub async fn get_confluence_pages_updated_since(
        &self,
        creds: &AtlassianCredentials,
        space_id: &str,
        since: &str,
    ) -> Result<ConfluenceGetPagesResponse> {
        let auth_header = creds.get_basic_auth_header();
        let url = format!("{}/wiki/api/v2/spaces/{}/pages", creds.base_url, space_id);

        debug!(
            "Searching Confluence pages updated since {}: {}",
            since, url
        );

        let client = self.client.clone();
        self.make_request(move || {
            client
                .get(&url)
                .header("Authorization", &auth_header)
                .header("Accept", "application/json")
        })
        .await
    }

    pub async fn get_jira_issues(
        &self,
        creds: &AtlassianCredentials,
        jql: &str,
        max_results: u32,
        next_page_token: Option<&str>,
        fields: &[String],
    ) -> Result<JiraSearchResponse> {
        let auth_header = creds.get_basic_auth_header();
        let url = format!("{}/rest/api/3/search/jql", creds.base_url);

        let fields_str = fields.join(",");
        let max_results_str = max_results.to_string();
        let mut params = vec![
            ("jql", jql.to_string()),
            ("maxResults", max_results_str),
            ("fields", fields_str),
            ("expand", "renderedFields".to_string()),
        ];

        if let Some(token) = next_page_token {
            params.push(("nextPageToken", token.to_string()));
        }

        debug!("Searching JIRA issues with JQL: {}", jql);

        let client = self.client.clone();
        self.make_request(move || {
            client
                .get(&url)
                .header("Authorization", &auth_header)
                .header("Accept", "application/json")
                .query(&params)
        })
        .await
    }

    pub async fn get_jira_issue_by_key(
        &self,
        creds: &AtlassianCredentials,
        issue_key: &str,
        fields: &[String],
    ) -> Result<JiraIssue> {
        let auth_header = creds.get_basic_auth_header();
        let fields_param = if fields.is_empty() {
            "*all".to_string()
        } else {
            fields.join(",")
        };

        let url = format!(
            "{}/rest/api/3/issue/{}?fields={}&expand=renderedFields",
            creds.base_url,
            issue_key,
            urlencoding::encode(&fields_param)
        );

        debug!("Fetching JIRA issue: {}", url);

        let client = self.client.clone();
        self.make_request(move || {
            client
                .get(&url)
                .header("Authorization", &auth_header)
                .header("Accept", "application/json")
        })
        .await
    }

    pub async fn get_jira_fields(&self, creds: &AtlassianCredentials) -> Result<Vec<JiraField>> {
        let auth_header = creds.get_basic_auth_header();
        let url = format!("{}/rest/api/3/field", creds.base_url);

        debug!("Fetching JIRA fields: {}", url);

        let client = self.client.clone();
        self.make_request(move || {
            client
                .get(&url)
                .header("Authorization", &auth_header)
                .header("Accept", "application/json")
        })
        .await
    }

    pub async fn get_confluence_spaces(
        &self,
        creds: &AtlassianCredentials,
    ) -> Result<Vec<ConfluenceSpace>> {
        let auth_header = creds.get_basic_auth_header();
        let mut url = format!("{}/wiki/api/v2/spaces", creds.base_url);
        let page_size = 250;
        let params = vec![("limit", page_size.to_string())];

        let mut results: Vec<ConfluenceSpace> = vec![];
        loop {
            debug!("Fetching Confluence spaces: {}, params: {:?}", url, params);

            let client = self.client.clone();
            let resp: ConfluenceGetSpacesResponse = self
                .make_request(|| {
                    client
                        .get(&url)
                        .query(&params)
                        .header("Authorization", &auth_header)
                        .header("Accept", "application/json")
                })
                .await?;

            debug!("Fetched {} spaces from Confluence", resp.results.len());
            for space in resp.results {
                results.push(space)
            }

            debug!("Confluence get spaces response links: {:?}", resp.links);
            if let Some(links) = resp.links {
                if let Some(next) = links.next {
                    let base_url = links.base;
                    debug!(
                        "Next page of spaces available, base: {}, next: {:?}",
                        base_url, next
                    );
                    url = format!("{}{}", base_url, next)
                } else {
                    debug!("All spaces fetched, returning.");
                    return Ok(results);
                }
            } else {
                debug!("No links in response, returning.");
                return Ok(results);
            }
        }
    }

    pub async fn get_jira_projects(
        &self,
        creds: &AtlassianCredentials,
        expand: &[&str],
    ) -> Result<Vec<serde_json::Value>> {
        let auth_header = creds.get_basic_auth_header();
        let mut url = format!("{}/rest/api/3/project", creds.base_url);

        if !expand.is_empty() {
            url.push_str(&format!("?expand={}", expand.join(",")));
        }

        debug!("Fetching JIRA projects: {}", url);

        let client = self.client.clone();
        self.make_request(move || {
            client
                .get(&url)
                .header("Authorization", &auth_header)
                .header("Accept", "application/json")
        })
        .await
    }
}
