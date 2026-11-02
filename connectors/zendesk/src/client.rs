use std::collections::HashSet;
use std::fmt;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use omni_connector_sdk::{RateLimiter, RetryableError};
use reqwest::header::RETRY_AFTER;
use reqwest::redirect::Policy;
use reqwest::{Client, Response, StatusCode};
use serde::de::DeserializeOwned;

use crate::config::ZendeskConfig;
use crate::credentials::ZendeskCredentials;
use crate::models::{
    Comment, CommentsResponse, GroupMembership, GroupMembershipsResponse, MeResponse, Paged,
    Person, TicketExportResponse, User, UsersResponse,
};

const REQUESTS_PER_SECOND: u32 = 5;
const MAX_RETRIES: u32 = 5;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_BODY_BYTES: usize = 50 * 1024 * 1024;
const MAX_ERROR_BODY_CHARS: usize = 200;
const MAX_RETRY_AFTER_SECS: u64 = 60;
const DEFAULT_RETRY_AFTER_SECS: u64 = 10;
const PAGE_SIZE: &str = "100";

/// A non-success Zendesk response. Carries `METHOD /path` and the status, never request headers.
#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    label: String,
    body: Option<String>,
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} failed with HTTP {}", self.label, self.status)?;
        match &self.body {
            Some(body) => write!(f, ": {body}"),
            None => Ok(()),
        }
    }
}

impl std::error::Error for ApiError {}

impl ApiError {
    fn new(status: StatusCode, label: &str, body: &[u8]) -> Self {
        // Auth failures drop the body entirely: it is of no use and may echo the credential.
        let body = if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
            None
        } else {
            Some(
                String::from_utf8_lossy(body)
                    .chars()
                    .take(MAX_ERROR_BODY_CHARS)
                    .collect(),
            )
        };
        Self {
            status,
            label: label.to_owned(),
            body,
        }
    }
}

#[derive(Clone)]
pub struct ZendeskClient {
    http: Client,
    base_url: String,
    credentials: ZendeskCredentials,
    limiter: RateLimiter,
}

/// Walks a cursor-paginated list. The next request is built from the cursor value alone,
/// never from a server-supplied URL.
pub struct Pager {
    path: String,
    params: Vec<(&'static str, String)>,
    after: Option<String>,
    seen: HashSet<String>,
    done: bool,
}

impl Pager {
    fn new(path: impl Into<String>, params: Vec<(&'static str, String)>) -> Self {
        Self {
            path: path.into(),
            params,
            after: None,
            seen: HashSet::new(),
            done: false,
        }
    }

    pub async fn next<R: Paged>(&mut self, client: &ZendeskClient) -> Result<Option<R>> {
        if self.done {
            return Ok(None);
        }
        let mut params = self.params.clone();
        params.push(("page[size]", PAGE_SIZE.to_string()));
        if let Some(after) = &self.after {
            params.push(("page[after]", after.clone()));
        }
        let page: R = client.get(&self.path, &params).await?;
        if page.meta().has_more {
            let cursor = page
                .meta()
                .after_cursor
                .clone()
                .with_context(|| format!("GET {}: has_more without after_cursor", self.path))?;
            if !self.seen.insert(cursor.clone()) {
                bail!("GET {}: Zendesk repeated a pagination cursor", self.path);
            }
            self.after = Some(cursor);
        } else {
            self.done = true;
        }
        Ok(Some(page))
    }
}

impl ZendeskClient {
    pub fn new(config: &ZendeskConfig, credentials: ZendeskCredentials) -> Result<Self> {
        Self::with_base_url(config.api_base_url(), credentials)
    }

    pub fn with_base_url(base_url: String, credentials: ZendeskCredentials) -> Result<Self> {
        credentials.validate()?;
        let http = Client::builder()
            .redirect(Policy::none())
            .timeout(REQUEST_TIMEOUT)
            .build()?;
        Ok(Self {
            http,
            base_url,
            credentials,
            limiter: RateLimiter::new(REQUESTS_PER_SECOND, MAX_RETRIES),
        })
    }

    async fn get<T: DeserializeOwned>(&self, path: &str, params: &[(&str, String)]) -> Result<T> {
        let label = format!("GET {path}");
        let body = self
            .limiter
            .execute_with_retry(|| self.send(path, params, &label))
            .await?;
        serde_json::from_slice(&body).with_context(|| format!("{label}: unexpected response"))
    }

    async fn send(
        &self,
        path: &str,
        params: &[(&str, String)],
        label: &str,
    ) -> Result<Vec<u8>, RetryableError> {
        let request = self.http.get(format!("{}{path}", self.base_url));
        let request = match &self.credentials {
            ZendeskCredentials::ApiToken { email, api_token } => {
                request.basic_auth(format!("{email}/token"), Some(api_token))
            }
            ZendeskCredentials::Bearer { access_token } => request.bearer_auth(access_token),
        };
        let response = request
            .query(params)
            .send()
            .await
            .map_err(|e| RetryableError::Transient(e.without_url().into()))?;

        let status = response.status();
        if status == StatusCode::TOO_MANY_REQUESTS {
            let retry_after = retry_after(&response);
            return Err(RetryableError::RateLimited {
                retry_after,
                message: label.to_owned(),
            });
        }
        let body = read_body(response)
            .await
            .map_err(RetryableError::Transient)?;
        if status.is_success() {
            Ok(body)
        } else if status.is_server_error() {
            Err(RetryableError::Transient(
                ApiError::new(status, label, &body).into(),
            ))
        } else {
            Err(RetryableError::Permanent(
                ApiError::new(status, label, &body).into(),
            ))
        }
    }

    pub async fn me(&self) -> Result<MeResponse> {
        self.get("/users/me.json", &[]).await
    }

    pub async fn agents_and_admins(&self) -> Result<Vec<User>> {
        let mut pager = Pager::new(
            "/users.json",
            vec![("role[]", "admin".into()), ("role[]", "agent".into())],
        );
        let mut users = Vec::new();
        while let Some(page) = pager.next::<UsersResponse>(self).await? {
            users.extend(page.users);
        }
        Ok(users)
    }

    pub async fn group_memberships(&self) -> Result<Vec<GroupMembership>> {
        let mut pager = Pager::new("/group_memberships.json", Vec::new());
        let mut memberships = Vec::new();
        while let Some(page) = pager.next::<GroupMembershipsResponse>(self).await? {
            memberships.extend(page.group_memberships);
        }
        Ok(memberships)
    }

    /// Newest-updated first, so an incremental walk can stop early.
    pub fn articles_pager(&self) -> Pager {
        Pager::new(
            "/help_center/articles.json",
            vec![
                ("sort_by", "updated_at".into()),
                ("sort_order", "desc".into()),
            ],
        )
    }

    pub async fn ticket_comments(&self, ticket_id: u64) -> Result<(Vec<Comment>, Vec<Person>)> {
        let mut pager = Pager::new(
            format!("/tickets/{ticket_id}/comments.json"),
            vec![("include", "users".into())],
        );
        let (mut comments, mut users) = (Vec::new(), Vec::new());
        while let Some(page) = pager.next::<CommentsResponse>(self).await? {
            comments.extend(page.comments);
            users.extend(page.users);
        }
        Ok((comments, users))
    }

    /// One page of the cursor-based incremental ticket export; `None` starts from the beginning.
    pub async fn export_tickets(&self, cursor: Option<&str>) -> Result<TicketExportResponse> {
        let start = match cursor {
            Some(cursor) => ("cursor", cursor.to_owned()),
            None => ("start_time", "0".to_owned()),
        };
        self.get(
            "/incremental/tickets/cursor.json",
            &[start, ("include", "users,groups,organizations".to_owned())],
        )
        .await
    }
}

/// True when Zendesk rejected the credential itself rather than failing.
pub fn is_auth_rejection(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<ApiError>()
        .is_some_and(|e| matches!(e.status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN))
}

fn retry_after(response: &Response) -> Duration {
    let secs = response
        .headers()
        .get(RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_RETRY_AFTER_SECS);
    Duration::from_secs(secs.min(MAX_RETRY_AFTER_SECS))
}

async fn read_body(mut response: Response) -> Result<Vec<u8>> {
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|e| e.without_url())? {
        if body.len() + chunk.len() > MAX_BODY_BYTES {
            return Err(anyhow!("Zendesk response exceeded {MAX_BODY_BYTES} bytes"));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}
