use anyhow::{anyhow, Result};
use async_trait::async_trait;
use futures::stream::Stream;
use omni_connector_sdk::{RateLimiter, RetryableError};
use reqwest::{Client, Response, StatusCode};
use serde::de::DeserializeOwned;
use std::pin::Pin;
use std::time::Duration;
use tracing::debug;

use crate::auth::AtlassianCredentials;
use crate::models::{
    AtlassianWebhookRegistration, AtlassianWebhookRegistrationResponse,
    ConfluenceContentRestriction, ConfluenceCqlPage, ConfluenceCqlSearchResponse,
    ConfluenceGetPagesResponse, ConfluenceGetSpacesResponse, ConfluenceGroupMembersResponse,
    ConfluencePage, ConfluenceSpace, ConfluenceSpacePermission, ConfluenceSpacePermissionsResponse,
    JiraField, JiraGroupMembersResponse, JiraIssue, JiraIssueSecuritySchemeResponse,
    JiraPermissionSchemeResponse, JiraProjectIssueSecuritySchemeResponse, JiraProjectRolesResponse,
    JiraRoleActorsResponse, JiraSearchResponse, JiraSecurityLevelMember,
    JiraSecurityLevelMembersResponse, OrgAdminGroupMembersResponse, OrgAdminGroupsResponse,
    OrgAdminUsersResponse,
};
use std::collections::HashMap;

#[async_trait]
pub trait AtlassianApi: Send + Sync {
    fn get_confluence_pages<'a>(
        &'a self,
        creds: &'a AtlassianCredentials,
        space_id: &'a str,
    ) -> Pin<Box<dyn Stream<Item = Result<ConfluencePage>> + Send + 'a>>;

    fn search_confluence_pages_by_cql<'a>(
        &'a self,
        creds: &'a AtlassianCredentials,
        cql: &'a str,
    ) -> Pin<Box<dyn Stream<Item = Result<ConfluenceCqlPage>> + Send + 'a>>;

    async fn get_confluence_spaces(
        &self,
        creds: &AtlassianCredentials,
    ) -> Result<Vec<ConfluenceSpace>>;

    async fn get_confluence_page_by_id(
        &self,
        creds: &AtlassianCredentials,
        page_id: &str,
        expand: &[&str],
    ) -> Result<ConfluencePage>;

    async fn get_jira_issues(
        &self,
        creds: &AtlassianCredentials,
        jql: &str,
        max_results: u32,
        next_page_token: Option<&str>,
        fields: &[String],
    ) -> Result<JiraSearchResponse>;

    async fn get_jira_issue_by_key(
        &self,
        creds: &AtlassianCredentials,
        issue_key: &str,
        fields: &[String],
    ) -> Result<JiraIssue>;

    async fn get_jira_fields(&self, creds: &AtlassianCredentials) -> Result<Vec<JiraField>>;

    async fn get_jira_projects(
        &self,
        creds: &AtlassianCredentials,
        expand: &[&str],
    ) -> Result<Vec<serde_json::Value>>;

    async fn register_webhook(
        &self,
        creds: &AtlassianCredentials,
        webhook_url: &str,
    ) -> Result<u64>;

    async fn delete_webhook(&self, creds: &AtlassianCredentials, webhook_id: u64) -> Result<()>;

    async fn get_webhook(&self, creds: &AtlassianCredentials, webhook_id: u64) -> Result<bool>;

    async fn get_confluence_space_permissions(
        &self,
        creds: &AtlassianCredentials,
        space_id: &str,
    ) -> Result<Vec<ConfluenceSpacePermission>>;

    async fn get_confluence_group_members(
        &self,
        creds: &AtlassianCredentials,
        group_id: &str,
    ) -> Result<Vec<String>>;

    async fn get_jira_group_members(
        &self,
        creds: &AtlassianCredentials,
        group_id: &str,
    ) -> Result<Vec<String>>;

    async fn get_jira_project_role_actors(
        &self,
        creds: &AtlassianCredentials,
        project_key: &str,
        role_id: &str,
    ) -> Result<JiraRoleActorsResponse>;

    async fn get_jira_project_roles(
        &self,
        creds: &AtlassianCredentials,
        project_key: &str,
    ) -> Result<JiraProjectRolesResponse>;

    async fn get_jira_users_bulk(
        &self,
        creds: &AtlassianCredentials,
        account_ids: &[String],
    ) -> Result<Vec<(String, String)>>;

    /// Returns the read-restriction principals for a Confluence page if any
    /// are set; returns Ok(None) when the page has no read restriction (the
    /// space-level perms apply unchanged).
    async fn get_confluence_page_read_restrictions(
        &self,
        creds: &AtlassianCredentials,
        page_id: &str,
    ) -> Result<Option<PageReadRestrictions>>;

    /// Returns the issue security scheme attached to a project, or Ok(None)
    /// if the project has no security scheme configured (any issue is
    /// readable to anyone with project access).
    async fn get_project_issue_security_scheme(
        &self,
        creds: &AtlassianCredentials,
        project_key: &str,
    ) -> Result<Option<JiraProjectIssueSecuritySchemeResponse>>;

    /// Returns the full issue security scheme (including all levels) by id.
    async fn get_issue_security_scheme(
        &self,
        creds: &AtlassianCredentials,
        scheme_id: &str,
    ) -> Result<JiraIssueSecuritySchemeResponse>;

    /// Paginates the holders of a single issue security level.
    async fn get_issue_security_level_members(
        &self,
        creds: &AtlassianCredentials,
        scheme_id: &str,
        level_id: &str,
    ) -> Result<Vec<JiraSecurityLevelMember>>;

    /// Returns the project's full permission scheme with all grants expanded.
    /// Use this to walk holders for `BROWSE_PROJECTS` beyond projectRole
    /// actors (i.e., direct user/group/anyone/applicationRole grants).
    async fn get_project_permission_scheme(
        &self,
        creds: &AtlassianCredentials,
        project_key: &str,
    ) -> Result<JiraPermissionSchemeResponse>;

    /// Returns `accountId → email` for every active user in the organization
    /// directory. Bypasses Atlassian Cloud's per-user email-privacy setting.
    /// Returns an empty map when org-admin credentials are not configured on
    /// `creds`. Requires Atlassian Guard / org-admin API key.
    async fn get_org_user_directory(
        &self,
        creds: &AtlassianCredentials,
    ) -> Result<HashMap<String, String>>;

    /// Returns `groupId → (name, member_account_ids)` for every group in the
    /// organization directory. Empty map when org-admin not configured.
    async fn get_org_group_directory(
        &self,
        creds: &AtlassianCredentials,
    ) -> Result<HashMap<String, OrgGroupInfo>>;
}

/// Group entry as returned by the Atlassian organization-admin API: the
/// canonical name plus the full member list (by accountId). Used to skip the
/// per-group member API calls when org-admin is configured.
#[derive(Debug, Clone)]
pub struct OrgGroupInfo {
    pub name: Option<String>,
    pub member_account_ids: Vec<String>,
}

/// Restriction principals applied to a Confluence page's read operation.
/// At least one of `user_account_ids` or `group_ids` is non-empty when this
/// struct is returned.
#[derive(Debug, Clone)]
pub struct PageReadRestrictions {
    pub user_account_ids: Vec<String>,
    pub group_ids: Vec<String>,
}

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
                        let body = response
                            .text()
                            .await
                            .map_err(|e| RetryableError::Transient(e.into()))?;
                        match serde_json::from_str::<T>(&body) {
                            Ok(data) => Ok(data),
                            Err(e) => Err(RetryableError::Permanent(anyhow!(
                                "Failed to decode response body: {} — body: {}",
                                e,
                                body
                            ))),
                        }
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
}

#[async_trait]
impl AtlassianApi for AtlassianClient {
    fn get_confluence_pages<'a>(
        &'a self,
        creds: &'a AtlassianCredentials,
        space_id: &'a str,
    ) -> Pin<Box<dyn Stream<Item = Result<ConfluencePage>> + Send + 'a>> {
        Box::pin(async_stream::stream! {
            let auth_header = creds.get_bearer_auth_header();
            let mut url = format!("{}/api/v2/spaces/{}/pages", creds.confluence_base(), space_id);
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

    async fn get_confluence_page_by_id(
        &self,
        creds: &AtlassianCredentials,
        page_id: &str,
        expand: &[&str],
    ) -> Result<ConfluencePage> {
        let auth_header = creds.get_bearer_auth_header();
        let mut url = format!("{}/rest/api/content/{}", creds.confluence_base(), page_id);

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

    fn search_confluence_pages_by_cql<'a>(
        &'a self,
        creds: &'a AtlassianCredentials,
        cql: &'a str,
    ) -> Pin<Box<dyn Stream<Item = Result<ConfluenceCqlPage>> + Send + 'a>> {
        Box::pin(async_stream::stream! {
            let auth_header = creds.get_bearer_auth_header();
            let url = format!("{}/rest/api/content/search", creds.confluence_base());
            let page_size = 50;
            let mut start = 0;

            loop {
                debug!("Searching Confluence pages with CQL: {} (start={})", cql, start);

                let client = self.client.clone();
                let params = vec![
                    ("cql", cql.to_string()),
                    ("limit", page_size.to_string()),
                    ("start", start.to_string()),
                    ("expand", "body.storage,version,space".to_string()),
                ];

                let resp: Result<ConfluenceCqlSearchResponse> = self
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

                debug!("CQL search returned {} results (start={})", resp.size, start);

                let result_count = resp.results.len();
                for page in resp.results {
                    yield Ok(page);
                }

                if (result_count as i64) < resp.limit {
                    return;
                }
                start += result_count as i64;
            }
        })
    }

    async fn get_jira_issues(
        &self,
        creds: &AtlassianCredentials,
        jql: &str,
        max_results: u32,
        next_page_token: Option<&str>,
        fields: &[String],
    ) -> Result<JiraSearchResponse> {
        let auth_header = creds.get_bearer_auth_header();
        let url = format!("{}/rest/api/3/search/jql", creds.jira_base());

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

    async fn get_jira_issue_by_key(
        &self,
        creds: &AtlassianCredentials,
        issue_key: &str,
        fields: &[String],
    ) -> Result<JiraIssue> {
        let auth_header = creds.get_bearer_auth_header();
        let fields_param = if fields.is_empty() {
            "*all".to_string()
        } else {
            fields.join(",")
        };

        let url = format!(
            "{}/rest/api/3/issue/{}?fields={}&expand=renderedFields",
            creds.jira_base(),
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

    async fn get_jira_fields(&self, creds: &AtlassianCredentials) -> Result<Vec<JiraField>> {
        let auth_header = creds.get_bearer_auth_header();
        let url = format!("{}/rest/api/3/field", creds.jira_base());

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

    async fn get_confluence_spaces(
        &self,
        creds: &AtlassianCredentials,
    ) -> Result<Vec<ConfluenceSpace>> {
        let auth_header = creds.get_bearer_auth_header();
        let mut url = format!("{}/api/v2/spaces", creds.confluence_base());
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

    async fn get_jira_projects(
        &self,
        creds: &AtlassianCredentials,
        expand: &[&str],
    ) -> Result<Vec<serde_json::Value>> {
        let auth_header = creds.get_bearer_auth_header();
        let mut url = format!("{}/rest/api/3/project", creds.jira_base());

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

    async fn register_webhook(
        &self,
        creds: &AtlassianCredentials,
        webhook_url: &str,
    ) -> Result<u64> {
        let auth_header = creds.get_bearer_auth_header();
        let url = format!("{}/rest/webhooks/1.0/webhook", creds.site_base());

        let registration = AtlassianWebhookRegistration {
            name: "Omni Atlassian Connector".to_string(),
            url: webhook_url.to_string(),
            events: vec![
                "jira:issue_created".to_string(),
                "jira:issue_updated".to_string(),
                "jira:issue_deleted".to_string(),
                "page_created".to_string(),
                "page_updated".to_string(),
                "page_removed".to_string(),
                "page_trashed".to_string(),
            ],
            enabled: true,
        };

        debug!("Registering Atlassian webhook: {}", url);

        let client = self.client.clone();
        let resp: AtlassianWebhookRegistrationResponse = self
            .make_request(move || {
                client
                    .post(&url)
                    .header("Authorization", &auth_header)
                    .header("Accept", "application/json")
                    .header("Content-Type", "application/json")
                    .json(&registration)
            })
            .await?;

        let webhook_id = resp
            .self_url
            .rsplit('/')
            .next()
            .and_then(|id| id.parse::<u64>().ok())
            .ok_or_else(|| {
                anyhow!(
                    "Failed to parse webhook ID from response: {}",
                    resp.self_url
                )
            })?;

        debug!("Registered webhook with ID: {}", webhook_id);
        Ok(webhook_id)
    }

    async fn delete_webhook(&self, creds: &AtlassianCredentials, webhook_id: u64) -> Result<()> {
        let auth_header = creds.get_bearer_auth_header();
        let url = format!(
            "{}/rest/webhooks/1.0/webhook/{}",
            creds.site_base(),
            webhook_id
        );

        debug!("Deleting Atlassian webhook {}: {}", webhook_id, url);

        let client = self.client.clone();
        self.rate_limiter
            .execute_with_retry(|| async {
                let response = client
                    .delete(&url)
                    .header("Authorization", &auth_header)
                    .send()
                    .await
                    .map_err(|e| RetryableError::Transient(e.into()))?;

                match response.status() {
                    StatusCode::OK | StatusCode::NO_CONTENT => Ok(()),
                    StatusCode::NOT_FOUND => Ok(()),
                    StatusCode::TOO_MANY_REQUESTS => {
                        let retry_after = Self::extract_retry_after(&response);
                        Err(RetryableError::RateLimited {
                            retry_after,
                            message: "Rate limited".to_string(),
                        })
                    }
                    _ => {
                        let status = response.status();
                        let text = response.text().await.unwrap_or_default();
                        Err(RetryableError::Permanent(anyhow!(
                            "Failed to delete webhook: HTTP {} - {}",
                            status,
                            text
                        )))
                    }
                }
            })
            .await
    }

    async fn get_webhook(&self, creds: &AtlassianCredentials, webhook_id: u64) -> Result<bool> {
        let auth_header = creds.get_bearer_auth_header();
        let url = format!(
            "{}/rest/webhooks/1.0/webhook/{}",
            creds.site_base(),
            webhook_id
        );

        debug!("Checking Atlassian webhook {}: {}", webhook_id, url);

        let client = self.client.clone();
        let result: Result<serde_json::Value> = self
            .make_request(move || {
                client
                    .get(&url)
                    .header("Authorization", &auth_header)
                    .header("Accept", "application/json")
            })
            .await;

        match result {
            Ok(_) => Ok(true),
            Err(e) => {
                let err_str = e.to_string();
                if err_str.contains("not found") || err_str.contains("404") {
                    Ok(false)
                } else {
                    Err(e)
                }
            }
        }
    }

    async fn get_confluence_space_permissions(
        &self,
        creds: &AtlassianCredentials,
        space_id: &str,
    ) -> Result<Vec<ConfluenceSpacePermission>> {
        let auth_header = creds.get_bearer_auth_header();
        let mut all_permissions = Vec::new();
        let mut url = format!(
            "{}/api/v2/spaces/{}/permissions",
            creds.confluence_base(),
            space_id
        );
        let params = vec![("limit", "100".to_string())];

        loop {
            debug!(
                "Fetching Confluence space {} permissions: {}",
                space_id, url
            );

            let client = self.client.clone();
            let resp: ConfluenceSpacePermissionsResponse = self
                .make_request(|| {
                    client
                        .get(&url)
                        .query(&params)
                        .header("Authorization", &auth_header)
                        .header("Accept", "application/json")
                })
                .await?;

            all_permissions.extend(resp.results);

            if let Some(links) = resp.links {
                if let Some(next) = links.next {
                    url = format!("{}{}", links.base, next);
                } else {
                    break;
                }
            } else {
                break;
            }
        }

        Ok(all_permissions)
    }

    async fn get_confluence_group_members(
        &self,
        creds: &AtlassianCredentials,
        group_id: &str,
    ) -> Result<Vec<String>> {
        let auth_header = creds.get_bearer_auth_header();
        let url = format!(
            "{}/rest/api/group/{}/membersByGroupId",
            creds.confluence_base(),
            group_id
        );
        let page_size: i64 = 200;
        let mut start: i64 = 0;
        let mut all_account_ids = Vec::new();

        loop {
            let params = vec![
                ("limit", page_size.to_string()),
                ("start", start.to_string()),
            ];

            debug!(
                "Fetching Confluence group {} members (start={})",
                group_id, start
            );

            let client = self.client.clone();
            let resp: ConfluenceGroupMembersResponse = self
                .make_request(|| {
                    client
                        .get(&url)
                        .query(&params)
                        .header("Authorization", &auth_header)
                        .header("Accept", "application/json")
                })
                .await?;

            let result_count = resp.results.len() as i64;
            all_account_ids.extend(resp.results.into_iter().map(|m| m.account_id));

            if result_count < resp.limit {
                break;
            }
            start += result_count;
        }

        Ok(all_account_ids)
    }

    async fn get_jira_group_members(
        &self,
        creds: &AtlassianCredentials,
        group_id: &str,
    ) -> Result<Vec<String>> {
        let auth_header = creds.get_bearer_auth_header();
        let url = format!("{}/rest/api/3/group/member", creds.jira_base());
        let page_size: u32 = 50;
        let mut start_at: u32 = 0;
        let mut all_account_ids = Vec::new();

        loop {
            let params = vec![
                ("groupId", group_id.to_string()),
                ("maxResults", page_size.to_string()),
                ("startAt", start_at.to_string()),
            ];

            debug!(
                "Fetching JIRA group {} members (startAt={})",
                group_id, start_at
            );

            let client = self.client.clone();
            let resp: JiraGroupMembersResponse = self
                .make_request(|| {
                    client
                        .get(&url)
                        .query(&params)
                        .header("Authorization", &auth_header)
                        .header("Accept", "application/json")
                })
                .await?;

            let result_count = resp.values.len() as u32;
            all_account_ids.extend(resp.values.into_iter().map(|m| m.account_id));

            if resp.is_last || result_count == 0 {
                break;
            }
            start_at += result_count;
        }

        Ok(all_account_ids)
    }

    async fn get_jira_project_roles(
        &self,
        creds: &AtlassianCredentials,
        project_key: &str,
    ) -> Result<JiraProjectRolesResponse> {
        let auth_header = creds.get_bearer_auth_header();
        let url = format!(
            "{}/rest/api/3/project/{}/role",
            creds.jira_base(),
            project_key
        );

        debug!("Fetching JIRA project {} roles", project_key);

        let client = self.client.clone();
        self.make_request(move || {
            client
                .get(&url)
                .header("Authorization", &auth_header)
                .header("Accept", "application/json")
        })
        .await
    }

    async fn get_jira_project_role_actors(
        &self,
        creds: &AtlassianCredentials,
        project_key: &str,
        role_id: &str,
    ) -> Result<JiraRoleActorsResponse> {
        let auth_header = creds.get_bearer_auth_header();
        let url = format!(
            "{}/rest/api/3/project/{}/role/{}",
            creds.jira_base(),
            project_key,
            role_id
        );

        debug!(
            "Fetching JIRA project {} role {} actors",
            project_key, role_id
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

    async fn get_jira_users_bulk(
        &self,
        creds: &AtlassianCredentials,
        account_ids: &[String],
    ) -> Result<Vec<(String, String)>> {
        if account_ids.is_empty() {
            return Ok(vec![]);
        }

        let auth_header = creds.get_bearer_auth_header();
        let mut results = Vec::new();

        // Jira bulk user API accepts up to 10 accountIds per request
        for chunk in account_ids.chunks(10) {
            let url = format!("{}/rest/api/3/user/bulk", creds.jira_base());
            let params: Vec<(&str, String)> =
                chunk.iter().map(|id| ("accountId", id.clone())).collect();

            debug!("Fetching {} users in bulk", chunk.len());

            let client = self.client.clone();
            let resp: crate::models::AtlassianUserBulkResponse = self
                .make_request(|| {
                    client
                        .get(&url)
                        .query(&params)
                        .header("Authorization", &auth_header)
                        .header("Accept", "application/json")
                })
                .await?;

            for user in resp.values {
                // Skip the service account — its email is not a meaningful
                // permission grant for end-user authz.
                if creds.sa_account_id.as_ref() == Some(&user.account_id) {
                    continue;
                }
                if let Some(email) = user.email_address {
                    results.push((user.account_id, email));
                }
            }
        }

        Ok(results)
    }

    async fn get_confluence_page_read_restrictions(
        &self,
        creds: &AtlassianCredentials,
        page_id: &str,
    ) -> Result<Option<PageReadRestrictions>> {
        let auth_header = creds.get_bearer_auth_header();
        let url = format!(
            "{}/rest/api/content/{}/restriction/byOperation/read",
            creds.confluence_base(),
            page_id
        );

        debug!("Fetching Confluence page {} read restrictions", page_id);

        let client = self.client.clone();
        let resp: ConfluenceContentRestriction = self
            .make_request(move || {
                client
                    .get(&url)
                    .query(&[("expand", "restrictions.user,restrictions.group")])
                    .header("Authorization", &auth_header)
                    .header("Accept", "application/json")
            })
            .await?;

        let user_account_ids: Vec<String> = resp
            .restrictions
            .user
            .results
            .into_iter()
            .map(|u| u.account_id)
            .collect();
        let group_ids: Vec<String> = resp
            .restrictions
            .group
            .results
            .into_iter()
            .map(|g| g.id)
            .collect();

        if user_account_ids.is_empty() && group_ids.is_empty() {
            return Ok(None);
        }

        Ok(Some(PageReadRestrictions {
            user_account_ids,
            group_ids,
        }))
    }

    async fn get_project_issue_security_scheme(
        &self,
        creds: &AtlassianCredentials,
        project_key: &str,
    ) -> Result<Option<JiraProjectIssueSecuritySchemeResponse>> {
        let auth_header = creds.get_bearer_auth_header();
        let url = format!(
            "{}/rest/api/3/project/{}/issuesecuritylevelscheme",
            creds.jira_base(),
            project_key
        );

        debug!("Fetching issue security scheme for project {}", project_key);

        let client = self.client.clone();
        let response = client
            .get(&url)
            .header("Authorization", &auth_header)
            .header("Accept", "application/json")
            .send()
            .await?;

        // Atlassian returns 404 when the project has no security scheme
        // attached — that's expected; treat as None.
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(anyhow!(
                "Failed to fetch issue security scheme for project {}: HTTP {} — {}",
                project_key,
                status,
                body
            ));
        }

        let scheme: JiraProjectIssueSecuritySchemeResponse = response.json().await?;
        Ok(Some(scheme))
    }

    async fn get_issue_security_scheme(
        &self,
        creds: &AtlassianCredentials,
        scheme_id: &str,
    ) -> Result<JiraIssueSecuritySchemeResponse> {
        let auth_header = creds.get_bearer_auth_header();
        let url = format!(
            "{}/rest/api/3/issuesecurityschemes/{}",
            creds.jira_base(),
            scheme_id
        );

        debug!("Fetching issue security scheme {} detail", scheme_id);

        let client = self.client.clone();
        self.make_request(move || {
            client
                .get(&url)
                .header("Authorization", &auth_header)
                .header("Accept", "application/json")
        })
        .await
    }

    async fn get_issue_security_level_members(
        &self,
        creds: &AtlassianCredentials,
        scheme_id: &str,
        level_id: &str,
    ) -> Result<Vec<JiraSecurityLevelMember>> {
        let auth_header = creds.get_bearer_auth_header();
        let url = format!(
            "{}/rest/api/3/issuesecurityschemes/{}/members",
            creds.jira_base(),
            scheme_id
        );
        let page_size: i64 = 50;
        let mut start_at: i64 = 0;
        let mut all = Vec::new();

        loop {
            debug!(
                "Fetching members for security scheme {} level {} (startAt={})",
                scheme_id, level_id, start_at
            );

            let client = self.client.clone();
            let resp: JiraSecurityLevelMembersResponse = self
                .make_request(|| {
                    client
                        .get(&url)
                        .query(&[
                            ("issueSecurityLevelId", level_id.to_string()),
                            ("maxResults", page_size.to_string()),
                            ("startAt", start_at.to_string()),
                            ("expand", "all".to_string()),
                        ])
                        .header("Authorization", &auth_header)
                        .header("Accept", "application/json")
                })
                .await?;

            let count = resp.values.len() as i64;
            let is_last = resp.is_last;
            all.extend(resp.values);

            if is_last || count == 0 {
                break;
            }
            start_at += count;
        }

        Ok(all)
    }

    async fn get_project_permission_scheme(
        &self,
        creds: &AtlassianCredentials,
        project_key: &str,
    ) -> Result<JiraPermissionSchemeResponse> {
        let auth_header = creds.get_bearer_auth_header();
        let url = format!(
            "{}/rest/api/3/project/{}/permissionscheme",
            creds.jira_base(),
            project_key
        );

        debug!("Fetching permission scheme for project {}", project_key);

        let client = self.client.clone();
        self.make_request(move || {
            client
                .get(&url)
                .query(&[("expand", "permissions,user,group")])
                .header("Authorization", &auth_header)
                .header("Accept", "application/json")
        })
        .await
    }

    async fn get_org_user_directory(
        &self,
        creds: &AtlassianCredentials,
    ) -> Result<HashMap<String, String>> {
        let (Some(org_id), Some(bearer)) = (&creds.org_id, creds.get_org_admin_bearer_header())
        else {
            return Ok(HashMap::new());
        };

        let mut directory = HashMap::new();
        let mut next_url = Some(format!(
            "https://api.atlassian.com/admin/v1/orgs/{}/users",
            org_id
        ));

        while let Some(url) = next_url.take() {
            debug!("Fetching org user directory page: {}", url);
            let client = self.client.clone();
            let bearer_clone = bearer.clone();
            let resp: OrgAdminUsersResponse = self
                .make_request(move || {
                    client
                        .get(&url)
                        .header("Authorization", &bearer_clone)
                        .header("Accept", "application/json")
                })
                .await?;

            for user in resp.data {
                let active = user
                    .account_status
                    .as_deref()
                    .map(|s| s.eq_ignore_ascii_case("active"))
                    .unwrap_or(true);
                if !active {
                    continue;
                }
                if let Some(email) = user.email {
                    directory.insert(user.account_id, email);
                }
            }

            next_url = resp.links.and_then(|l| l.next);
        }

        Ok(directory)
    }

    async fn get_org_group_directory(
        &self,
        creds: &AtlassianCredentials,
    ) -> Result<HashMap<String, OrgGroupInfo>> {
        let (Some(org_id), Some(bearer)) = (&creds.org_id, creds.get_org_admin_bearer_header())
        else {
            return Ok(HashMap::new());
        };

        let mut directory: HashMap<String, OrgGroupInfo> = HashMap::new();
        let mut next_url = Some(format!(
            "https://api.atlassian.com/admin/v1/orgs/{}/groups",
            org_id
        ));

        while let Some(url) = next_url.take() {
            debug!("Fetching org group directory page: {}", url);
            let client = self.client.clone();
            let bearer_clone = bearer.clone();
            let resp: OrgAdminGroupsResponse = self
                .make_request(move || {
                    client
                        .get(&url)
                        .header("Authorization", &bearer_clone)
                        .header("Accept", "application/json")
                })
                .await?;

            for group in resp.data {
                directory.insert(
                    group.id,
                    OrgGroupInfo {
                        name: group.name,
                        member_account_ids: vec![],
                    },
                );
            }

            next_url = resp.links.and_then(|l| l.next);
        }

        // Fetch members for each group. Cursor-paginated like the others.
        for (group_id, info) in directory.iter_mut() {
            let mut next = Some(format!(
                "https://api.atlassian.com/admin/v1/orgs/{}/groups/{}/members",
                org_id, group_id
            ));
            while let Some(url) = next.take() {
                debug!("Fetching members for org group {} ({})", group_id, url);
                let client = self.client.clone();
                let bearer_clone = bearer.clone();
                let resp: OrgAdminGroupMembersResponse = self
                    .make_request(move || {
                        client
                            .get(&url)
                            .header("Authorization", &bearer_clone)
                            .header("Accept", "application/json")
                    })
                    .await?;
                for member in resp.data {
                    info.member_account_ids.push(member.account_id);
                }
                next = resp.links.and_then(|l| l.next);
            }
        }

        Ok(directory)
    }
}
