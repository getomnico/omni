use chrono::DateTime;
use serde::{Deserialize, Serialize};
use shared::models::{ConnectorEvent, DocumentMetadata, DocumentPermissions};
use sqlx::types::time::OffsetDateTime;
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfluencePage {
    pub id: String,
    pub title: String,
    pub r#type: String, // page, blogpost
    pub status: String, // current, trashed, draft
    pub space: ConfluenceSpace,
    pub version: ConfluenceVersion,
    pub body: Option<ConfluenceBody>,
    pub ancestors: Option<Vec<ConfluenceAncestor>>,
    #[serde(rename = "_links")]
    pub links: Option<ConfluenceLinks>,
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
    pub by: ConfluenceUser,
    pub when: String,
    pub number: i32,
    pub message: Option<String>,
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
pub struct ConfluenceBody {
    pub storage: Option<ConfluenceContent>,
    pub view: Option<ConfluenceContent>,
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
pub struct ConfluenceSearchResponse {
    pub results: Vec<ConfluencePage>,
    pub start: i32,
    pub limit: i32,
    pub size: i32,
    #[serde(rename = "_links")]
    pub links: Option<ConfluenceResponseLinks>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ConfluenceResponseLinks {
    pub base: String,
    pub context: String,
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

#[derive(Debug, Serialize, Deserialize)]
pub struct JiraSearchResponse {
    pub issues: Vec<JiraIssue>,
    pub total: i32,
    #[serde(rename = "startAt")]
    pub start_at: i32,
    #[serde(rename = "maxResults")]
    pub max_results: i32,
}

impl ConfluencePage {
    pub fn extract_plain_text(&self) -> String {
        let mut content = String::new();

        if let Some(body) = &self.body {
            if let Some(storage) = &body.storage {
                content = self.strip_html_tags(&storage.value);
            } else if let Some(view) = &body.view {
                content = self.strip_html_tags(&view.value);
            }
        }

        content.trim().to_string()
    }

    fn strip_html_tags(&self, html: &str) -> String {
        // Simple HTML tag stripping - in production, consider using a proper HTML parser
        let re = regex::Regex::new(r"<[^>]*>").unwrap();
        re.replace_all(html, " ")
            .into_owned()
            .split_whitespace()
            .collect::<Vec<&str>>()
            .join(" ")
    }

    pub fn to_connector_event(
        &self,
        sync_run_id: String,
        source_id: String,
        base_url: &str,
    ) -> ConnectorEvent {
        let document_id = format!("confluence_page_{}_{}", self.space.key, self.id);

        let created_at = DateTime::parse_from_rfc3339(&self.version.when)
            .ok()
            .map(|dt| {
                OffsetDateTime::from_unix_timestamp(dt.timestamp())
                    .unwrap_or(OffsetDateTime::UNIX_EPOCH)
            });

        let mut extra = HashMap::new();
        extra.insert(
            "space_id".to_string(),
            serde_json::Value::String(self.space.id.clone()),
        );
        extra.insert(
            "space_key".to_string(),
            serde_json::Value::String(self.space.key.clone()),
        );
        extra.insert(
            "space_name".to_string(),
            serde_json::Value::String(self.space.name.clone()),
        );
        extra.insert(
            "page_type".to_string(),
            serde_json::Value::String(self.r#type.clone()),
        );
        extra.insert(
            "status".to_string(),
            serde_json::Value::String(self.status.clone()),
        );
        extra.insert(
            "version".to_string(),
            serde_json::Value::Number(self.version.number.into()),
        );

        if let Some(ancestors) = &self.ancestors {
            let ancestor_titles: Vec<String> = ancestors.iter().map(|a| a.title.clone()).collect();
            extra.insert("ancestors".to_string(), serde_json::json!(ancestor_titles));
        }

        let url = if let Some(links) = &self.links {
            Some(format!("{}{}", base_url, links.web_ui))
        } else {
            Some(format!(
                "{}/wiki/spaces/{}/pages/{}",
                base_url, self.space.key, self.id
            ))
        };

        let metadata = DocumentMetadata {
            title: Some(self.title.clone()),
            author: Some(self.version.by.display_name.clone()),
            created_at,
            updated_at: created_at,
            mime_type: Some("text/html".to_string()),
            size: Some(self.extract_plain_text().len().to_string()),
            url,
            parent_id: self
                .ancestors
                .as_ref()
                .and_then(|a| a.last().map(|p| p.id.clone())),
            extra: Some(extra),
        };

        let permissions = DocumentPermissions {
            public: false,
            users: vec![],
            groups: vec![format!("confluence_space_{}", self.space.key)],
        };

        ConnectorEvent::DocumentCreated {
            sync_run_id,
            source_id,
            document_id,
            content: self.extract_plain_text(),
            metadata,
            permissions,
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

    pub fn to_document_content(&self) -> String {
        let mut content = format!("Summary: {}\n\n", self.fields.summary);

        let description = self.extract_description_text();
        if !description.is_empty() {
            content.push_str(&format!("Description:\n{}\n\n", description));
        }

        content.push_str(&format!("Issue Type: {}\n", self.fields.issuetype.name));
        content.push_str(&format!("Status: {}\n", self.fields.status.name));
        content.push_str(&format!("Project: {}\n", self.fields.project.name));

        if let Some(priority) = &self.fields.priority {
            content.push_str(&format!("Priority: {}\n", priority.name));
        }

        if let Some(assignee) = &self.fields.assignee {
            content.push_str(&format!("Assignee: {}\n", assignee.display_name));
        }

        if let Some(reporter) = &self.fields.reporter {
            content.push_str(&format!("Reporter: {}\n", reporter.display_name));
        }

        if let Some(labels) = &self.fields.labels {
            if !labels.is_empty() {
                content.push_str(&format!("Labels: {}\n", labels.join(", ")));
            }
        }

        let comments = self.extract_comments_text();
        if !comments.is_empty() {
            content.push_str(&format!("\nComments:\n{}", comments));
        }

        content
    }

    pub fn to_connector_event(
        &self,
        sync_run_id: String,
        source_id: String,
        base_url: &str,
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
        extra.insert(
            "issue_key".to_string(),
            serde_json::Value::String(self.key.clone()),
        );
        extra.insert(
            "project_id".to_string(),
            serde_json::Value::String(self.fields.project.id.clone()),
        );
        extra.insert(
            "project_key".to_string(),
            serde_json::Value::String(self.fields.project.key.clone()),
        );
        extra.insert(
            "project_name".to_string(),
            serde_json::Value::String(self.fields.project.name.clone()),
        );
        extra.insert(
            "issue_type".to_string(),
            serde_json::Value::String(self.fields.issuetype.name.clone()),
        );
        extra.insert(
            "status".to_string(),
            serde_json::Value::String(self.fields.status.name.clone()),
        );
        extra.insert(
            "status_category".to_string(),
            serde_json::Value::String(self.fields.status.status_category.name.clone()),
        );

        if let Some(priority) = &self.fields.priority {
            extra.insert(
                "priority".to_string(),
                serde_json::Value::String(priority.name.clone()),
            );
        }

        if let Some(labels) = &self.fields.labels {
            extra.insert("labels".to_string(), serde_json::json!(labels));
        }

        let url = Some(format!("{}/browse/{}", base_url, self.key));

        let metadata = DocumentMetadata {
            title: Some(format!("{} - {}", self.key, self.fields.summary)),
            author: self.fields.creator.as_ref().map(|c| c.display_name.clone()),
            created_at,
            updated_at,
            mime_type: Some("text/plain".to_string()),
            size: Some(self.to_document_content().len().to_string()),
            url,
            parent_id: None,
            extra: Some(extra),
        };

        let permissions = DocumentPermissions {
            public: false,
            users: vec![],
            groups: vec![format!("jira_project_{}", self.fields.project.key)],
        };

        ConnectorEvent::DocumentCreated {
            sync_run_id,
            source_id,
            document_id,
            content: self.to_document_content(),
            metadata,
            permissions,
        }
    }
}
