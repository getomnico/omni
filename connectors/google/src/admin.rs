use anyhow::{anyhow, Result};
use reqwest::Client;
use serde::Deserialize;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, info};

use shared::RateLimiter;

const ADMIN_API_BASE: &str = "https://admin.googleapis.com/admin/directory/v1";

#[derive(Debug, Clone, Deserialize)]
pub struct User {
    pub id: String,
    #[serde(rename = "primaryEmail")]
    pub primary_email: String,
    pub name: Option<UserName>,
    #[serde(rename = "isAdmin")]
    pub is_admin: Option<bool>,
    pub suspended: Option<bool>,
    #[serde(rename = "orgUnitPath")]
    pub org_unit_path: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UserName {
    #[serde(rename = "givenName")]
    pub given_name: Option<String>,
    #[serde(rename = "familyName")]
    pub family_name: Option<String>,
    #[serde(rename = "fullName")]
    pub full_name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UsersListResponse {
    pub users: Option<Vec<User>>,
    #[serde(rename = "nextPageToken")]
    pub next_page_token: Option<String>,
}

pub struct AdminClient {
    client: Client,
    rate_limiter: Option<Arc<RateLimiter>>,
}

impl AdminClient {
    pub fn new() -> Self {
        let client = Client::builder()
            .pool_max_idle_per_host(5) // Reuse connections for admin API requests
            .pool_idle_timeout(Duration::from_secs(90))
            .tcp_keepalive(Duration::from_secs(60))
            .timeout(Duration::from_secs(60))
            .connect_timeout(Duration::from_secs(10))
            .build()
            .expect("Failed to build HTTP client");

        Self {
            client,
            rate_limiter: None,
        }
    }

    pub fn with_rate_limiter(rate_limiter: Arc<RateLimiter>) -> Self {
        let client = Client::builder()
            .pool_max_idle_per_host(5) // Reuse connections for admin API requests
            .pool_idle_timeout(Duration::from_secs(90))
            .tcp_keepalive(Duration::from_secs(60))
            .timeout(Duration::from_secs(60))
            .connect_timeout(Duration::from_secs(10))
            .build()
            .expect("Failed to build HTTP client");

        Self {
            client,
            rate_limiter: Some(rate_limiter),
        }
    }

    pub async fn search_users(
        &self,
        token: &str,
        domain: &str,
        query: Option<&str>,
        max_results: Option<u32>,
        page_token: Option<&str>,
    ) -> Result<UsersListResponse> {
        let search_users_impl = || async {
            let url = format!("{}/users", ADMIN_API_BASE);

            let max_results_str = max_results.unwrap_or(50).to_string();
            let mut params = vec![
                ("domain", domain),
                ("maxResults", &max_results_str),
                ("orderBy", "email"),
            ];

            let search_query_opt = query.map(|q| q.to_string());

            if let Some(ref search_query) = search_query_opt {
                params.push(("query", search_query));
            }

            if let Some(token) = page_token {
                params.push(("pageToken", token));
            }

            let response = self
                .client
                .get(&url)
                .bearer_auth(token)
                .query(&params)
                .send()
                .await?;

            if !response.status().is_success() {
                let error_text = response.text().await?;
                return Err(anyhow!("Failed to search users: {}", error_text));
            }

            debug!("Admin API search response status: {}", response.status());
            let response_text = response.text().await?;
            debug!("Admin API search raw response: {}", response_text);

            serde_json::from_str(&response_text).map_err(|e| {
                anyhow!(
                    "Failed to parse Admin API search response: {}. Raw response: {}",
                    e,
                    response_text
                )
            })
        };

        match &self.rate_limiter {
            Some(limiter) => {
                limiter
                    .execute_with_retry(|| async { search_users_impl().await.map_err(Into::into) })
                    .await
            }
            None => search_users_impl().await,
        }
    }

    pub async fn list_all_users(&self, token: &str, domain: &str) -> Result<Vec<User>> {
        let mut all_users = Vec::new();
        let mut page_token: Option<String> = None;

        loop {
            let response = self
                .list_users(token, domain, page_token.as_deref())
                .await?;

            if let Some(users) = response.users {
                let active_users: Vec<User> = users
                    .into_iter()
                    .filter(|u| !u.suspended.unwrap_or(false))
                    .collect();

                info!("Found {} active users in this page", active_users.len());

                all_users.extend(active_users);
            }

            page_token = response.next_page_token;
            if page_token.is_none() {
                break;
            }
        }

        info!("Total active users found: {}", all_users.len());
        Ok(all_users)
    }

    async fn list_users(
        &self,
        token: &str,
        domain: &str,
        page_token: Option<&str>,
    ) -> Result<UsersListResponse> {
        let list_users_impl = || async {
            let url = format!("{}/users", ADMIN_API_BASE);

            let mut params = vec![
                ("domain", domain),
                ("maxResults", "200"),
                ("orderBy", "email"),
            ];

            if let Some(token) = page_token {
                params.push(("pageToken", token));
            }

            let response = self
                .client
                .get(&url)
                .bearer_auth(token)
                .query(&params)
                .send()
                .await?;

            if !response.status().is_success() {
                let error_text = response.text().await?;
                return Err(anyhow!("Failed to list users: {}", error_text));
            }

            debug!("Admin API response status: {}", response.status());
            let response_text = response.text().await?;
            debug!("Admin API raw response: {}", response_text);

            serde_json::from_str(&response_text).map_err(|e| {
                anyhow!(
                    "Failed to parse Admin API response: {}. Raw response: {}",
                    e,
                    response_text
                )
            })
        };

        match &self.rate_limiter {
            Some(limiter) => {
                limiter
                    .execute_with_retry(|| async { list_users_impl().await.map_err(Into::into) })
                    .await
            }
            None => list_users_impl().await,
        }
    }

    pub async fn list_all_groups(&self, token: &str, domain: &str) -> Result<Vec<Group>> {
        let mut all_groups = Vec::new();
        let mut page_token: Option<String> = None;

        loop {
            let response = self
                .list_groups(token, domain, page_token.as_deref())
                .await?;

            if let Some(groups) = response.groups {
                info!("Found {} groups in this page", groups.len());
                all_groups.extend(groups);
            }

            page_token = response.next_page_token;
            if page_token.is_none() {
                break;
            }
        }

        info!("Total groups found: {}", all_groups.len());
        Ok(all_groups)
    }

    async fn list_groups(
        &self,
        token: &str,
        domain: &str,
        page_token: Option<&str>,
    ) -> Result<GroupsListResponse> {
        let list_groups_impl = || async {
            let url = format!("{}/groups", ADMIN_API_BASE);

            let mut params = vec![("domain", domain), ("maxResults", "200")];

            if let Some(token) = page_token {
                params.push(("pageToken", token));
            }

            let response = self
                .client
                .get(&url)
                .bearer_auth(token)
                .query(&params)
                .send()
                .await?;

            if !response.status().is_success() {
                let error_text = response.text().await?;
                return Err(anyhow!("Failed to list groups: {}", error_text));
            }

            let response_text = response.text().await?;
            debug!("Admin API groups response: {}", response_text);

            serde_json::from_str(&response_text).map_err(|e| {
                anyhow!(
                    "Failed to parse groups response: {}. Raw: {}",
                    e,
                    response_text
                )
            })
        };

        match &self.rate_limiter {
            Some(limiter) => {
                limiter
                    .execute_with_retry(|| async { list_groups_impl().await.map_err(Into::into) })
                    .await
            }
            None => list_groups_impl().await,
        }
    }

    pub async fn list_all_group_members(
        &self,
        token: &str,
        group_key: &str,
    ) -> Result<Vec<GroupMember>> {
        let mut all_members = Vec::new();
        let mut page_token: Option<String> = None;

        loop {
            let response = self
                .list_group_members(token, group_key, page_token.as_deref())
                .await?;

            if let Some(members) = response.members {
                let active_members: Vec<GroupMember> = members
                    .into_iter()
                    .filter(|m| m.status.as_deref() != Some("SUSPENDED"))
                    .collect();
                all_members.extend(active_members);
            }

            page_token = response.next_page_token;
            if page_token.is_none() {
                break;
            }
        }

        Ok(all_members)
    }

    async fn list_group_members(
        &self,
        token: &str,
        group_key: &str,
        page_token: Option<&str>,
    ) -> Result<MembersListResponse> {
        let list_members_impl = || async {
            let url = format!("{}/groups/{}/members", ADMIN_API_BASE, group_key);

            let mut params = vec![("maxResults", "200")];

            if let Some(token) = page_token {
                params.push(("pageToken", token));
            }

            let response = self
                .client
                .get(&url)
                .bearer_auth(token)
                .query(&params)
                .send()
                .await?;

            if !response.status().is_success() {
                let error_text = response.text().await?;
                return Err(anyhow!(
                    "Failed to list group members for {}: {}",
                    group_key,
                    error_text
                ));
            }

            let response_text = response.text().await?;

            serde_json::from_str(&response_text).map_err(|e| {
                anyhow!(
                    "Failed to parse members response: {}. Raw: {}",
                    e,
                    response_text
                )
            })
        };

        match &self.rate_limiter {
            Some(limiter) => {
                limiter
                    .execute_with_retry(|| async { list_members_impl().await.map_err(Into::into) })
                    .await
            }
            None => list_members_impl().await,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Group {
    pub id: String,
    pub email: String,
    pub name: Option<String>,
    pub description: Option<String>,
    #[serde(rename = "directMembersCount")]
    pub direct_members_count: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct GroupsListResponse {
    pub groups: Option<Vec<Group>>,
    #[serde(rename = "nextPageToken")]
    pub next_page_token: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GroupMember {
    pub id: Option<String>,
    pub email: Option<String>,
    pub role: Option<String>,
    #[serde(rename = "type")]
    pub member_type: Option<String>,
    pub status: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct MembersListResponse {
    pub members: Option<Vec<GroupMember>>,
    #[serde(rename = "nextPageToken")]
    pub next_page_token: Option<String>,
}
