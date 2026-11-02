use omni_connector_sdk::SyncType;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

pub const CHECKPOINT_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Users,
    Articles,
    Tickets,
    Done,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZendeskCheckpoint {
    pub version: u32,
    /// Mode of the run that wrote this checkpoint.
    pub mode: SyncType,
    /// Unix seconds at run start; stable across resume.
    pub cutoff: i64,
    pub phase: Phase,
    /// `after_cursor` of the last ticket export page that was fully emitted.
    pub tickets_cursor: Option<String>,
    /// Max `updated_at` (unix seconds) seen by the last completed articles phase.
    pub articles_watermark: Option<i64>,
    /// Article ids indexed so far, for delete reconciliation on full sync.
    pub known_article_ids: Vec<u64>,
    /// Zendesk group ids emitted by the last run, so groups that disappeared can be emptied.
    #[serde(default)]
    pub known_group_ids: Vec<u64>,
}

#[derive(Debug, Deserialize)]
pub struct PageMeta {
    pub has_more: bool,
    pub after_cursor: Option<String>,
}

/// A cursor-paginated list response.
pub trait Paged: serde::de::DeserializeOwned {
    fn meta(&self) -> &PageMeta;
}

#[derive(Debug, Deserialize)]
pub struct MeResponse {
    pub user: MeUser,
}

/// `id` is null when the request was not authenticated.
#[derive(Debug, Deserialize)]
pub struct MeUser {
    pub id: Option<u64>,
    pub email: Option<String>,
    pub role: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct User {
    pub id: u64,
    pub name: Option<String>,
    pub email: Option<String>,
    pub role: String,
    pub ticket_restriction: Option<String>,
    pub active: bool,
    pub suspended: bool,
}

#[derive(Debug, Deserialize)]
pub struct UsersResponse {
    pub users: Vec<User>,
    pub meta: PageMeta,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GroupMembership {
    pub user_id: u64,
    pub group_id: u64,
}

#[derive(Debug, Deserialize)]
pub struct GroupMembershipsResponse {
    pub group_memberships: Vec<GroupMembership>,
    pub meta: PageMeta,
}

/// A sideloaded user: only the fields needed to render names.
#[derive(Debug, Clone, Deserialize)]
pub struct Person {
    pub id: u64,
    pub name: Option<String>,
    pub email: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Named {
    pub id: u64,
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct Via {
    pub channel: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Ticket {
    pub id: u64,
    pub subject: Option<String>,
    pub description: Option<String>,
    pub status: String,
    pub priority: Option<String>,
    #[serde(rename = "type")]
    pub ticket_type: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub group_id: Option<u64>,
    pub organization_id: Option<u64>,
    pub requester_id: Option<u64>,
    pub assignee_id: Option<u64>,
    pub via: Option<Via>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Deserialize)]
pub struct TicketExportResponse {
    pub tickets: Vec<Ticket>,
    #[serde(default)]
    pub users: Vec<Person>,
    #[serde(default)]
    pub groups: Vec<Named>,
    #[serde(default)]
    pub organizations: Vec<Named>,
    pub after_cursor: Option<String>,
    pub end_of_stream: bool,
}

#[derive(Debug, Deserialize)]
pub struct Attachment {
    pub file_name: String,
}

#[derive(Debug, Deserialize)]
pub struct Comment {
    pub author_id: u64,
    pub plain_body: Option<String>,
    pub public: bool,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
}

#[derive(Debug, Deserialize)]
pub struct CommentsResponse {
    pub comments: Vec<Comment>,
    #[serde(default)]
    pub users: Vec<Person>,
    pub meta: PageMeta,
}

#[derive(Debug, Deserialize)]
pub struct Article {
    pub id: u64,
    pub title: String,
    pub body: Option<String>,
    pub html_url: String,
    pub author_id: Option<u64>,
    pub section_id: Option<u64>,
    pub locale: String,
    pub draft: bool,
    pub user_segment_id: Option<u64>,
    /// Enterprise plans: several segments; empty means everyone.
    #[serde(default)]
    pub user_segment_ids: Vec<u64>,
    pub permission_group_id: Option<u64>,
    #[serde(default)]
    pub label_names: Vec<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Deserialize)]
pub struct ArticlesResponse {
    pub articles: Vec<Article>,
    pub meta: PageMeta,
}

macro_rules! impl_paged {
    ($($response:ty),+) => {
        $(impl Paged for $response {
            fn meta(&self) -> &PageMeta {
                &self.meta
            }
        })+
    };
}

impl_paged!(
    UsersResponse,
    GroupMembershipsResponse,
    CommentsResponse,
    ArticlesResponse
);
