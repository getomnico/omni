use anyhow::{anyhow, Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use crate::auth::{
    classify_google_api_error, execute_with_auth_retry, google_max_retries, ApiResult, GoogleAuth,
};
use omni_connector_sdk::RateLimiter;

const CHAT_API_BASE: &str = "https://chat.googleapis.com/v1";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum GoogleChatSpaceType {
    SpaceTypeUnspecified,
    Space,
    GroupChat,
    DirectMessage,
}

impl Default for GoogleChatSpaceType {
    fn default() -> Self {
        Self::SpaceTypeUnspecified
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoogleChatSpace {
    pub name: String,
    #[serde(default, rename = "spaceType")]
    pub space_type: GoogleChatSpaceType,
    #[serde(default, rename = "displayName")]
    pub display_name: Option<String>,
    #[serde(default, rename = "spaceUri")]
    pub space_uri: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ListSpacesResponse {
    #[serde(default)]
    pub spaces: Vec<GoogleChatSpace>,
    #[serde(rename = "nextPageToken")]
    pub next_page_token: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum GoogleChatUserType {
    TypeUnspecified,
    Human,
    Bot,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoogleChatUser {
    pub name: String,
    #[serde(default, rename = "displayName")]
    pub display_name: Option<String>,
    #[serde(default, rename = "type")]
    pub user_type: Option<GoogleChatUserType>,
    #[serde(default, rename = "isAnonymous")]
    pub is_anonymous: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoogleChatGroupMember {
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoogleChatMembership {
    pub name: String,
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub member: Option<GoogleChatUser>,
    #[serde(default, rename = "groupMember")]
    pub group_member: Option<GoogleChatGroupMember>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ListMembershipsResponse {
    #[serde(default)]
    pub memberships: Vec<GoogleChatMembership>,
    #[serde(rename = "nextPageToken")]
    pub next_page_token: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoogleChatThread {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default, rename = "threadKey")]
    pub thread_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoogleChatDriveDataRef {
    #[serde(rename = "driveFileId")]
    pub drive_file_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoogleChatAttachmentDataRef {
    #[serde(default, rename = "resourceName")]
    pub resource_name: Option<String>,
    #[serde(default, rename = "attachmentUploadToken")]
    pub attachment_upload_token: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum GoogleChatAttachmentSource {
    SourceUnspecified,
    DriveFile,
    UploadedContent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoogleChatAttachment {
    pub name: String,
    #[serde(default, rename = "contentName")]
    pub content_name: Option<String>,
    #[serde(default, rename = "contentType")]
    pub content_type: Option<String>,
    #[serde(default)]
    pub source: Option<GoogleChatAttachmentSource>,
    #[serde(default, rename = "attachmentDataRef")]
    pub attachment_data_ref: Option<GoogleChatAttachmentDataRef>,
    #[serde(default, rename = "driveDataRef")]
    pub drive_data_ref: Option<GoogleChatDriveDataRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoogleChatMessage {
    pub name: String,
    #[serde(default)]
    pub sender: Option<GoogleChatUser>,
    #[serde(default, rename = "createTime")]
    pub create_time: Option<String>,
    #[serde(default, rename = "lastUpdateTime")]
    pub last_update_time: Option<String>,
    #[serde(default, rename = "deleteTime")]
    pub delete_time: Option<String>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default, rename = "formattedText")]
    pub formatted_text: Option<String>,
    #[serde(default)]
    pub thread: Option<GoogleChatThread>,
    #[serde(default, rename = "threadReply")]
    pub thread_reply: Option<bool>,
    #[serde(default)]
    pub attachment: Vec<GoogleChatAttachment>,
    #[serde(default, rename = "privateMessageViewer")]
    pub private_message_viewer: Option<GoogleChatUser>,
    #[serde(default, rename = "quotedMessageMetadata")]
    pub quoted_message_metadata: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ListMessagesResponse {
    #[serde(default)]
    pub messages: Vec<GoogleChatMessage>,
    #[serde(rename = "nextPageToken")]
    pub next_page_token: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoogleChatSpaceEvent {
    pub name: String,
    #[serde(rename = "eventTime")]
    pub event_time: String,
    #[serde(rename = "eventType")]
    pub event_type: String,
    #[serde(default, rename = "messageCreatedEventData")]
    pub message_created: Option<GoogleChatMessageEventData>,
    #[serde(default, rename = "messageUpdatedEventData")]
    pub message_updated: Option<GoogleChatMessageEventData>,
    #[serde(default, rename = "messageDeletedEventData")]
    pub message_deleted: Option<GoogleChatMessageEventData>,
    #[serde(default, rename = "membershipCreatedEventData")]
    pub membership_created: Option<GoogleChatMembershipEventData>,
    #[serde(default, rename = "membershipUpdatedEventData")]
    pub membership_updated: Option<GoogleChatMembershipEventData>,
    #[serde(default, rename = "membershipDeletedEventData")]
    pub membership_deleted: Option<GoogleChatMembershipEventData>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoogleChatMessageEventData {
    pub message: GoogleChatMessage,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoogleChatMembershipEventData {
    pub membership: GoogleChatMembership,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ListSpaceEventsResponse {
    #[serde(default, rename = "spaceEvents")]
    pub space_events: Vec<GoogleChatSpaceEvent>,
    #[serde(rename = "nextPageToken")]
    pub next_page_token: Option<String>,
}

#[derive(Clone)]
pub struct ChatClient {
    client: Client,
    user_rate_limiters: Arc<RwLock<HashMap<String, Arc<RateLimiter>>>>,
}

impl ChatClient {
    pub fn with_rate_limiter(rate_limiter: Arc<RateLimiter>) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(60))
            .connect_timeout(Duration::from_secs(10))
            .pool_max_idle_per_host(10)
            .pool_idle_timeout(Duration::from_secs(90))
            .tcp_keepalive(Duration::from_secs(60))
            .build()
            .expect("Failed to build HTTP client");
        let _ = rate_limiter;
        Self {
            client,
            user_rate_limiters: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    fn get_or_create_user_rate_limiter(&self, user_email: &str) -> Result<Arc<RateLimiter>> {
        {
            let rate_limiters = self.user_rate_limiters.read().map_err(|e| {
                anyhow!(
                    "Failed to acquire read lock on Chat user rate limiters: {:?}",
                    e
                )
            })?;
            if let Some(limiter) = rate_limiters.get(user_email) {
                return Ok(limiter.clone());
            }
        }
        let mut rate_limiters = self.user_rate_limiters.write().map_err(|e| {
            anyhow!(
                "Failed to acquire write lock on Chat user rate limiters: {:?}",
                e
            )
        })?;
        Ok(rate_limiters
            .entry(user_email.to_string())
            .or_insert_with(|| Arc::new(RateLimiter::new(25, google_max_retries())))
            .clone())
    }

    pub async fn list_spaces_for_user(
        &self,
        auth: &GoogleAuth,
        user_email: &str,
        page_token: Option<&str>,
    ) -> Result<ListSpacesResponse> {
        let page_token = page_token.map(str::to_string);
        let limiter = self.get_or_create_user_rate_limiter(user_email)?;
        execute_with_auth_retry(auth, user_email, limiter, |token| {
            let page_token = page_token.clone();
            async move {
                let url = format!("{}/spaces", CHAT_API_BASE);
                let mut params = vec![("pageSize", "1000".to_string())];
                if let Some(token) = page_token {
                    params.push(("pageToken", token));
                }
                let response = self
                    .client
                    .get(&url)
                    .bearer_auth(&token)
                    .query(&params)
                    .send()
                    .await?;
                if !response.status().is_success() {
                    return classify_google_api_error(response, "Failed to list Chat spaces").await;
                }
                Ok(ApiResult::Success(
                    response
                        .json::<ListSpacesResponse>()
                        .await
                        .context("Failed to parse Chat spaces")?,
                ))
            }
        })
        .await
    }

    pub async fn list_members(
        &self,
        auth: &GoogleAuth,
        user_email: &str,
        space_name: &str,
        page_token: Option<&str>,
        show_groups: bool,
        use_admin_access: bool,
    ) -> Result<ListMembershipsResponse> {
        let space_name = space_name.to_string();
        let page_token = page_token.map(str::to_string);
        let limiter = self.get_or_create_user_rate_limiter(user_email)?;
        execute_with_auth_retry(auth, user_email, limiter, |token| {
            let space_name = space_name.clone();
            let page_token = page_token.clone();
            async move {
                let url = format!("{}/{}/members", CHAT_API_BASE, space_name);
                let mut params = vec![
                    ("pageSize", "1000".to_string()),
                    ("showGroups", show_groups.to_string()),
                    (
                        "filter",
                        "member.type = \"HUMAN\" OR member.type != \"BOT\"".to_string(),
                    ),
                ];
                if use_admin_access {
                    params.push(("useAdminAccess", "true".to_string()));
                }
                if let Some(token) = page_token {
                    params.push(("pageToken", token));
                }
                let response = self
                    .client
                    .get(&url)
                    .bearer_auth(&token)
                    .query(&params)
                    .send()
                    .await?;
                if !response.status().is_success() {
                    return classify_google_api_error(response, "Failed to list Chat memberships")
                        .await;
                }
                Ok(ApiResult::Success(
                    response
                        .json::<ListMembershipsResponse>()
                        .await
                        .context("Failed to parse Chat memberships")?,
                ))
            }
        })
        .await
    }

    pub async fn list_messages(
        &self,
        auth: &GoogleAuth,
        user_email: &str,
        space_name: &str,
        page_token: Option<&str>,
        filter: Option<&str>,
        order_by: Option<&str>,
        show_deleted: bool,
    ) -> Result<ListMessagesResponse> {
        let space_name = space_name.to_string();
        let page_token = page_token.map(str::to_string);
        let filter = filter.map(str::to_string);
        let order_by = order_by.map(str::to_string);
        let limiter = self.get_or_create_user_rate_limiter(user_email)?;
        execute_with_auth_retry(auth, user_email, limiter, |token| {
            let space_name = space_name.clone();
            let page_token = page_token.clone();
            let filter = filter.clone();
            let order_by = order_by.clone();
            async move {
                let url = format!("{}/{}/messages", CHAT_API_BASE, space_name);
                let mut params = vec![
                    ("pageSize", "1000".to_string()),
                    ("showDeleted", show_deleted.to_string()),
                ];
                if let Some(token) = page_token {
                    params.push(("pageToken", token));
                }
                if let Some(filter) = filter {
                    params.push(("filter", filter));
                }
                if let Some(order_by) = order_by {
                    params.push(("orderBy", order_by));
                }
                let response = self
                    .client
                    .get(&url)
                    .bearer_auth(&token)
                    .query(&params)
                    .send()
                    .await?;
                if !response.status().is_success() {
                    return classify_google_api_error(response, "Failed to list Chat messages")
                        .await;
                }
                Ok(ApiResult::Success(
                    response
                        .json::<ListMessagesResponse>()
                        .await
                        .context("Failed to parse Chat messages")?,
                ))
            }
        })
        .await
    }

    pub async fn get_message(
        &self,
        auth: &GoogleAuth,
        user_email: &str,
        message_name: &str,
    ) -> Result<GoogleChatMessage> {
        let message_name = message_name.to_string();
        let limiter = self.get_or_create_user_rate_limiter(user_email)?;
        execute_with_auth_retry(auth, user_email, limiter, |token| {
            let message_name = message_name.clone();
            async move {
                let url = format!("{}/{}", CHAT_API_BASE, message_name);
                let response = self.client.get(&url).bearer_auth(&token).send().await?;
                if !response.status().is_success() {
                    return classify_google_api_error(response, "Failed to get Chat message").await;
                }
                Ok(ApiResult::Success(
                    response
                        .json::<GoogleChatMessage>()
                        .await
                        .context("Failed to parse Chat message")?,
                ))
            }
        })
        .await
    }

    pub async fn list_space_events(
        &self,
        auth: &GoogleAuth,
        user_email: &str,
        space_name: &str,
        page_token: Option<&str>,
        filter: &str,
    ) -> Result<ListSpaceEventsResponse> {
        let space_name = space_name.to_string();
        let page_token = page_token.map(str::to_string);
        let filter = filter.to_string();
        let limiter = self.get_or_create_user_rate_limiter(user_email)?;
        execute_with_auth_retry(auth, user_email, limiter, |token| {
            let space_name = space_name.clone();
            let page_token = page_token.clone();
            let filter = filter.clone();
            async move {
                let url = format!("{}/{}/spaceEvents", CHAT_API_BASE, space_name);
                let mut params = vec![("filter", filter), ("pageSize", "1000".to_string())];
                if let Some(token) = page_token {
                    params.push(("pageToken", token));
                }
                let response = self
                    .client
                    .get(&url)
                    .bearer_auth(&token)
                    .query(&params)
                    .send()
                    .await?;
                if !response.status().is_success() {
                    return classify_google_api_error(response, "Failed to list Chat space events")
                        .await;
                }
                Ok(ApiResult::Success(
                    response
                        .json::<ListSpaceEventsResponse>()
                        .await
                        .context("Failed to parse Chat space events")?,
                ))
            }
        })
        .await
    }

    pub async fn download_uploaded_attachment(
        &self,
        auth: &GoogleAuth,
        user_email: &str,
        resource_name: &str,
    ) -> Result<Vec<u8>> {
        let resource_name = resource_name.to_string();
        let limiter = self.get_or_create_user_rate_limiter(user_email)?;
        execute_with_auth_retry(auth, user_email, limiter, |token| {
            let resource_name = resource_name.clone();
            async move {
                let url = format!(
                    "{}/media/{}",
                    CHAT_API_BASE,
                    urlencoding::encode(&resource_name)
                );
                let response = self
                    .client
                    .get(&url)
                    .bearer_auth(&token)
                    .query(&[("alt", "media")])
                    .send()
                    .await?;
                if !response.status().is_success() {
                    return classify_google_api_error(
                        response,
                        "Failed to download Chat attachment",
                    )
                    .await;
                }
                Ok(ApiResult::Success(response.bytes().await?.to_vec()))
            }
        })
        .await
    }
}
