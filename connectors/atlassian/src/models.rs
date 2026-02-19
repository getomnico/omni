use chrono::DateTime;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as JsonValue};
use shared::models::{ConnectorEvent, DocumentAttributes, DocumentMetadata, DocumentPermissions};
use std::collections::HashMap;
use time::OffsetDateTime;

// ============================================================================
// Connector Protocol Models
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectorManifest {
    pub name: String,
    pub version: String,
    pub sync_modes: Vec<String>,
    #[serde(default)]
    pub actions: Vec<ActionDefinition>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionDefinition {
    pub name: String,
    pub description: String,
    pub parameters: JsonValue,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncResponse {
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl SyncResponse {
    pub fn started() -> Self {
        Self {
            status: "started".to_string(),
            message: None,
        }
    }

    pub fn error(msg: impl Into<String>) -> Self {
        Self {
            status: "error".to_string(),
            message: Some(msg.into()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CancelRequest {
    pub sync_run_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CancelResponse {
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionRequest {
    pub action: String,
    pub params: JsonValue,
    pub credentials: JsonValue,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionResponse {
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<JsonValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl ActionResponse {
    pub fn not_supported(action: &str) -> Self {
        Self {
            status: "error".to_string(),
            result: None,
            error: Some(format!("Action not supported: {}", action)),
        }
    }
}

// ============================================================================
// Atlassian Models
// ============================================================================

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ConfluencePageStatus {
    Current,
    Draft,
    Archived,
    Historical,
    Trashed,
    Deleted,
    Any,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ConfluencePageParentType {
    Page,
    Whiteboard,
    Database,
    Embed,
    Folder,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Hash)]
pub struct ConfluencePageLinks {
    pub webui: String,
    pub editui: String,
    pub tinyui: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfluencePage {
    pub id: String,
    pub status: ConfluencePageStatus,
    pub title: String,
    #[serde(rename = "spaceId")]
    pub space_id: String,
    #[serde(rename = "parentId")]
    pub parent_id: Option<String>,
    pub parent_type: Option<ConfluencePageParentType>,
    pub position: Option<i32>,
    #[serde(rename = "authorId")]
    pub author_id: String,
    #[serde(rename = "ownerId")]
    pub owner_id: Option<String>,
    #[serde(rename = "lastOwnerId")]
    pub last_owner_id: Option<String>,
    pub subtype: Option<String>,
    #[serde(rename = "createdAt", with = "time::serde::iso8601")]
    pub created_at: OffsetDateTime,
    pub version: ConfluenceVersion,
    pub body: Option<ConfluencePageBody>,
    #[serde(rename = "_links")]
    pub links: ConfluencePageLinks,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfluenceSpace {
    pub id: String,
    pub key: String,
    pub name: String,
    pub r#type: String, // global, personal
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfluenceVersion {
    #[serde(rename = "createdAt", with = "time::serde::iso8601")]
    pub created_at: OffsetDateTime,
    pub message: String,
    pub number: i32,
    #[serde(rename = "minorEdit")]
    pub minor_edit: bool,
    #[serde(rename = "authorId")]
    pub author_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfluenceUser {
    #[serde(rename = "type")]
    pub user_type: String,
    #[serde(rename = "accountId")]
    pub account_id: String,
    #[serde(rename = "displayName")]
    pub display_name: String,
    pub email: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfluencePageBody {
    pub storage: Option<ConfluenceContent>,
    pub atlas_doc_format: Option<ConfluenceContent>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfluenceContent {
    pub value: String,
    pub representation: String, // storage, view, export_view
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfluenceAncestor {
    pub id: String,
    pub title: String,
    pub r#type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfluenceLinks {
    #[serde(rename = "webui")]
    pub web_ui: String,
    #[serde(rename = "self")]
    pub self_link: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ConfluenceGetPagesResponse {
    pub results: Vec<ConfluencePage>,
    #[serde(rename = "_links")]
    pub links: Option<ConfluenceResponseLinks>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ConfluenceGetSpacesResponse {
    pub results: Vec<ConfluenceSpace>,
    #[serde(rename = "_links")]
    pub links: Option<ConfluenceResponseLinks>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ConfluenceResponseLinks {
    pub base: String,
    pub next: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JiraIssue {
    pub id: String,
    pub key: String,
    #[serde(rename = "self")]
    pub self_url: String,
    pub fields: JiraFields,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JiraFields {
    pub summary: String,
    pub description: Option<JiraDescription>,
    pub issuetype: JiraIssueType,
    pub status: JiraStatus,
    pub priority: Option<JiraPriority>,
    pub assignee: Option<JiraUser>,
    pub reporter: Option<JiraUser>,
    pub creator: Option<JiraUser>,
    pub project: JiraProject,
    pub created: String,
    pub updated: String,
    pub labels: Option<Vec<String>>,
    pub comment: Option<JiraComments>,
    pub components: Option<Vec<JiraComponent>>,
    /// Captures custom fields (customfield_XXXXX) and any other unknown fields
    #[serde(flatten)]
    pub extra_fields: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JiraDescription {
    pub content: Vec<JiraContent>,
    #[serde(rename = "type")]
    pub content_type: String,
    pub version: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JiraContent {
    #[serde(rename = "type")]
    pub content_type: String,
    pub content: Option<Vec<JiraContent>>,
    pub text: Option<String>,
    pub attrs: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JiraIssueType {
    pub id: String,
    pub name: String,
    #[serde(rename = "iconUrl")]
    pub icon_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JiraStatus {
    pub id: String,
    pub name: String,
    #[serde(rename = "statusCategory")]
    pub status_category: JiraStatusCategory,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JiraStatusCategory {
    pub id: i32,
    pub name: String,
    pub key: String,
    #[serde(rename = "colorName")]
    pub color_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JiraPriority {
    pub id: String,
    pub name: String,
    #[serde(rename = "iconUrl")]
    pub icon_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JiraUser {
    #[serde(rename = "accountId")]
    pub account_id: String,
    #[serde(rename = "displayName")]
    pub display_name: String,
    #[serde(rename = "emailAddress")]
    pub email_address: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JiraProject {
    pub id: String,
    pub key: String,
    pub name: String,
    #[serde(rename = "avatarUrls")]
    pub avatar_urls: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JiraComments {
    pub comments: Vec<JiraComment>,
    pub total: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JiraComment {
    pub id: String,
    pub author: JiraUser,
    pub body: JiraDescription,
    pub created: String,
    pub updated: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JiraComponent {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
}

/// Structured attributes for JIRA issues, used for filtering and faceting.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JiraIssueAttributes {
    pub issue_key: String,
    pub issue_type: String,
    pub status: String,
    pub status_category: String,
    pub project_key: String,
    pub project_name: String,
    pub priority: Option<String>,
    pub assignee: Option<String>,
    pub assignee_email: Option<String>,
    pub reporter: Option<String>,
    pub reporter_email: Option<String>,
    pub labels: Vec<String>,
    pub components: Vec<String>,
    #[serde(flatten)]
    pub custom_fields: HashMap<String, serde_json::Value>,
}

impl JiraIssueAttributes {
    pub fn into_attributes(self) -> DocumentAttributes {
        let mut attrs = HashMap::new();
        attrs.insert("issue_key".into(), json!(self.issue_key));
        attrs.insert("issue_type".into(), json!(self.issue_type));
        attrs.insert("status".into(), json!(self.status));
        attrs.insert("status_category".into(), json!(self.status_category));
        attrs.insert("project_key".into(), json!(self.project_key));
        attrs.insert("project_name".into(), json!(self.project_name));
        if let Some(priority) = self.priority {
            attrs.insert("priority".into(), json!(priority));
        }
        if let Some(assignee) = self.assignee {
            attrs.insert("assignee".into(), json!(assignee));
        }
        if let Some(email) = self.assignee_email {
            attrs.insert("assignee_email".into(), json!(email));
        }
        if let Some(reporter) = self.reporter {
            attrs.insert("reporter".into(), json!(reporter));
        }
        if let Some(email) = self.reporter_email {
            attrs.insert("reporter_email".into(), json!(email));
        }
        if !self.labels.is_empty() {
            attrs.insert("labels".into(), json!(self.labels));
        }
        if !self.components.is_empty() {
            attrs.insert("components".into(), json!(self.components));
        }
        for (key, value) in self.custom_fields {
            if !value.is_null() {
                attrs.insert(key, value);
            }
        }
        attrs
    }
}

/// Structured attributes for Confluence pages, used for filtering and faceting.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfluencePageAttributes {
    pub space_id: String,
    pub status: String,
}

impl ConfluencePageAttributes {
    pub fn into_attributes(self) -> DocumentAttributes {
        let mut attrs = HashMap::new();
        attrs.insert("space_id".into(), json!(self.space_id));
        attrs.insert("status".into(), json!(self.status));
        attrs
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JiraField {
    pub id: String,
    pub name: String,
    pub custom: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct JiraSearchResponse {
    pub issues: Vec<JiraIssue>,
    #[serde(rename = "isLast", default)]
    pub is_last: bool,
    #[serde(rename = "nextPageToken")]
    pub next_page_token: Option<String>,
}

impl ConfluencePage {
    pub fn extract_plain_text(&self) -> String {
        let mut content = String::new();

        if let Some(body) = &self.body {
            if let Some(storage) = &body.storage {
                content = self.strip_html_tags(&storage.value);
            } else if let Some(doc) = &body.atlas_doc_format {
                content = self.strip_html_tags(&doc.value);
            }
        }

        content.trim().to_string()
    }

    fn strip_html_tags(&self, html: &str) -> String {
        let re = regex::Regex::new(r"<[^>]*>").unwrap();
        re.replace_all(html, " ")
            .into_owned()
            .split_whitespace()
            .collect::<Vec<&str>>()
            .join(" ")
    }

    pub fn to_attributes(&self) -> ConfluencePageAttributes {
        ConfluencePageAttributes {
            space_id: self.space_id.clone(),
            status: format!("{:?}", self.status).to_lowercase(),
        }
    }

    pub fn to_connector_event(
        &self,
        sync_run_id: String,
        source_id: String,
        base_url: &str,
        content_id: String,
    ) -> ConnectorEvent {
        let document_id = format!("confluence_page_{}_{}", self.space_id, self.id);
        let url = format!("{}/wiki{}", base_url, self.links.webui.clone());
        let path = self.title.clone();

        let mut extra = HashMap::new();
        let mut confluence_extra = HashMap::new();
        confluence_extra.insert("parent_id".to_string(), json!(self.parent_id));
        confluence_extra.insert("version".to_string(), json!(self.version.number));
        extra.insert("confluence".to_string(), json!(confluence_extra));

        let metadata = DocumentMetadata {
            title: Some(self.title.clone()),
            author: Some(self.author_id.clone()),
            created_at: Some(self.created_at),
            updated_at: Some(self.created_at),
            mime_type: Some("text/html".to_string()),
            size: Some(self.extract_plain_text().len().to_string()),
            url: Some(url),
            path: Some(path),
            extra: Some(extra),
        };

        let permissions = DocumentPermissions {
            public: true,
            users: vec![],
            groups: vec![],
        };

        let attributes = self.to_attributes().into_attributes();

        ConnectorEvent::DocumentCreated {
            sync_run_id,
            source_id,
            document_id,
            content_id,
            metadata,
            permissions,
            attributes: Some(attributes),
        }
    }
}

impl JiraIssue {
    pub fn extract_description_text(&self) -> String {
        self.fields
            .description
            .as_ref()
            .map(|desc| self.extract_text_from_content(&desc.content))
            .unwrap_or_default()
    }

    pub fn extract_comments_text(&self) -> String {
        if let Some(comments) = &self.fields.comment {
            comments
                .comments
                .iter()
                .map(|comment| {
                    let text = self.extract_text_from_content(&comment.body.content);
                    format!(
                        "{} ({}): {}",
                        comment.author.display_name, comment.created, text
                    )
                })
                .collect::<Vec<String>>()
                .join("\n\n")
        } else {
            String::new()
        }
    }

    fn extract_text_from_content(&self, content: &[JiraContent]) -> String {
        let mut text = String::new();

        for item in content {
            if let Some(item_text) = &item.text {
                text.push_str(item_text);
                text.push(' ');
            }

            if let Some(nested_content) = &item.content {
                text.push_str(&self.extract_text_from_content(nested_content));
            }
        }

        text.trim().to_string()
    }

    /// Generate textual content for FTS indexing and embeddings.
    /// Only includes human-written text, NOT structured fields.
    /// Structured fields go in `to_attributes()` for filtering.
    pub fn to_document_content(&self) -> String {
        let mut content = String::new();

        // Summary is the issue title - include it
        content.push_str(&self.fields.summary);
        content.push_str("\n\n");

        // Description is the main textual content
        let description = self.extract_description_text();
        if !description.is_empty() {
            content.push_str(&description);
            content.push_str("\n\n");
        }

        // Comments are user-written text content
        let comments = self.extract_comments_text();
        if !comments.is_empty() {
            content.push_str(&comments);
        }

        content.trim().to_string()
    }

    /// Generate structured attributes for filtering and faceting.
    pub fn to_attributes(&self) -> JiraIssueAttributes {
        JiraIssueAttributes {
            issue_key: self.key.clone(),
            issue_type: self.fields.issuetype.name.clone(),
            status: self.fields.status.name.clone(),
            status_category: self.fields.status.status_category.name.clone(),
            project_key: self.fields.project.key.clone(),
            project_name: self.fields.project.name.clone(),
            priority: self.fields.priority.as_ref().map(|p| p.name.clone()),
            assignee: self
                .fields
                .assignee
                .as_ref()
                .map(|a| a.display_name.clone()),
            assignee_email: self
                .fields
                .assignee
                .as_ref()
                .and_then(|a| a.email_address.clone()),
            reporter: self
                .fields
                .reporter
                .as_ref()
                .map(|r| r.display_name.clone()),
            reporter_email: self
                .fields
                .reporter
                .as_ref()
                .and_then(|r| r.email_address.clone()),
            labels: self.fields.labels.clone().unwrap_or_default(),
            components: self
                .fields
                .components
                .as_ref()
                .map(|c| c.iter().map(|comp| comp.name.clone()).collect())
                .unwrap_or_default(),
            custom_fields: self
                .fields
                .extra_fields
                .iter()
                .filter(|(_, v)| !v.is_null())
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        }
    }

    pub fn to_connector_event(
        &self,
        sync_run_id: String,
        source_id: String,
        base_url: &str,
        content_id: String,
    ) -> ConnectorEvent {
        let document_id = format!("jira_issue_{}_{}", self.fields.project.key, self.key);

        let created_at = DateTime::parse_from_rfc3339(&self.fields.created)
            .ok()
            .map(|dt| {
                OffsetDateTime::from_unix_timestamp(dt.timestamp())
                    .unwrap_or(OffsetDateTime::UNIX_EPOCH)
            });

        let updated_at = DateTime::parse_from_rfc3339(&self.fields.updated)
            .ok()
            .map(|dt| {
                OffsetDateTime::from_unix_timestamp(dt.timestamp())
                    .unwrap_or(OffsetDateTime::UNIX_EPOCH)
            });

        let mut extra = HashMap::new();
        let mut jira_extra = HashMap::new();
        jira_extra.insert("project_id".to_string(), json!(self.fields.project.id));
        extra.insert("jira".to_string(), json!(jira_extra));

        let url = Some(format!("{}/browse/{}", base_url, self.key));

        let metadata = DocumentMetadata {
            title: Some(format!("{} - {}", self.key, self.fields.summary)),
            author: self.fields.creator.as_ref().map(|c| c.display_name.clone()),
            created_at,
            updated_at,
            mime_type: Some("text/plain".to_string()),
            size: Some(self.to_document_content().len().to_string()),
            url,
            path: Some(format!("{}/{}", self.fields.project.name, self.key)),
            extra: Some(extra),
        };

        let permissions = DocumentPermissions {
            users: vec![],
            groups: vec![],
            public: true,
        };

        let attributes = self.to_attributes().into_attributes();

        ConnectorEvent::DocumentCreated {
            sync_run_id,
            source_id,
            document_id,
            content_id,
            metadata,
            permissions,
            attributes: Some(attributes),
        }
    }
}
