use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use shared::models::ConnectorEvent;
use shared::queue::EventQueue;
use tracing::{debug, error, info, warn};

use crate::auth::AtlassianCredentials;
use crate::client::AtlassianClient;
use crate::models::{JiraIssue, JiraSearchResponse};

pub struct JiraProcessor {
    client: AtlassianClient,
    event_queue: EventQueue,
}

impl JiraProcessor {
    pub fn new(event_queue: EventQueue) -> Self {
        Self {
            client: AtlassianClient::new(),
            event_queue,
        }
    }

    pub async fn sync_all_projects(
        &mut self,
        creds: &AtlassianCredentials,
        source_id: &str,
    ) -> Result<u32> {
        info!("Starting JIRA projects sync for source: {}", source_id);

        let projects = self.get_accessible_projects(creds).await?;
        let mut total_issues_processed = 0;

        for project in projects {
            let project_key = project
                .get("key")
                .and_then(|k| k.as_str())
                .ok_or_else(|| anyhow!("Project missing key"))?;

            let project_name = project
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or("Unknown Project");

            info!("Syncing JIRA project: {} ({})", project_name, project_key);

            match self
                .sync_project_issues(creds, source_id, project_key)
                .await
            {
                Ok(issues_count) => {
                    total_issues_processed += issues_count;
                    info!(
                        "Synced {} issues from project: {}",
                        issues_count, project_key
                    );
                }
                Err(e) => {
                    error!("Failed to sync project {}: {}", project_key, e);
                    // Continue with other projects
                }
            }
        }

        info!(
            "Completed JIRA sync. Total issues processed: {}",
            total_issues_processed
        );
        Ok(total_issues_processed)
    }

    pub async fn sync_issues_updated_since(
        &mut self,
        creds: &AtlassianCredentials,
        source_id: &str,
        since: DateTime<Utc>,
        project_key: Option<&str>,
    ) -> Result<u32> {
        info!(
            "Starting incremental JIRA sync for source: {} since {}{}",
            source_id,
            since.format("%Y-%m-%d %H:%M:%S"),
            project_key.map_or(String::new(), |p| format!(" (project: {})", p))
        );

        let since_str = since.format("%Y-%m-%d %H:%M").to_string();
        let mut total_issues = 0;
        let mut start_at = 0;
        const PAGE_SIZE: u32 = 50;

        loop {
            let response = self
                .client
                .get_jira_issues_updated_since(creds, &since_str, project_key, PAGE_SIZE, start_at)
                .await?;

            if response.issues.is_empty() {
                break;
            }

            let issues_count = response.issues.len();
            let events = self.process_issues(response.issues, source_id, &creds.base_url)?;
            self.queue_events(events).await?;

            total_issues += issues_count as u32;
            start_at += PAGE_SIZE;

            debug!(
                "Processed {} issues, total so far: {}",
                issues_count, total_issues
            );

            // Check if we've reached the end
            if issues_count < PAGE_SIZE as usize {
                break;
            }
        }

        info!(
            "Completed incremental JIRA sync. Issues processed: {}",
            total_issues
        );
        Ok(total_issues)
    }

    async fn sync_project_issues(
        &mut self,
        creds: &AtlassianCredentials,
        source_id: &str,
        project_key: &str,
    ) -> Result<u32> {
        let mut total_issues = 0;
        let mut start_at = 0;
        const PAGE_SIZE: u32 = 50;

        // JQL to get all issues in the project
        let jql = format!("project = {}", project_key);
        let fields = vec![
            "summary",
            "description",
            "issuetype",
            "status",
            "priority",
            "assignee",
            "reporter",
            "creator",
            "project",
            "created",
            "updated",
            "labels",
            "comment",
            "components",
        ];

        loop {
            let response = self
                .client
                .get_jira_issues(creds, &jql, PAGE_SIZE, start_at, &fields)
                .await?;

            if response.issues.is_empty() {
                break;
            }

            let issues_count = response.issues.len();
            let events = self.process_issues(response.issues, source_id, &creds.base_url)?;
            self.queue_events(events).await?;

            total_issues += issues_count as u32;
            start_at += PAGE_SIZE;

            debug!(
                "Processed {} issues from project {}, total: {}",
                issues_count, project_key, total_issues
            );

            // Check if we've reached the end
            if issues_count < PAGE_SIZE as usize {
                break;
            }
        }

        Ok(total_issues)
    }

    async fn get_accessible_projects(
        &mut self,
        creds: &AtlassianCredentials,
    ) -> Result<Vec<serde_json::Value>> {
        let expand = vec!["description", "lead", "issueTypes"];
        let projects = self.client.get_jira_projects(creds, &expand).await?;

        debug!("Found {} accessible JIRA projects", projects.len());
        Ok(projects)
    }

    fn process_issues(
        &self,
        issues: Vec<JiraIssue>,
        source_id: &str,
        base_url: &str,
    ) -> Result<Vec<ConnectorEvent>> {
        let mut events = Vec::new();

        for issue in issues {
            let content = issue.to_document_content();
            if content.trim().is_empty() {
                debug!("Skipping issue {} without content", issue.key);
                continue;
            }

            debug!(
                "Processing JIRA issue: {} - {} (content length: {} chars)",
                issue.key,
                issue.fields.summary,
                content.len()
            );

            // TODO: Add proper sync_run_id when sync runs are implemented for Atlassian
            let placeholder_sync_run_id = shared::utils::generate_ulid();
            let event =
                issue.to_connector_event(placeholder_sync_run_id, source_id.to_string(), base_url);
            events.push(event);
        }

        Ok(events)
    }

    async fn queue_events(&self, events: Vec<ConnectorEvent>) -> Result<()> {
        for event in events {
            if let Err(e) = self.event_queue.enqueue(event.source_id(), &event).await {
                error!("Failed to queue JIRA event: {}", e);
                // Continue processing other events
            }
        }
        Ok(())
    }

    pub async fn sync_single_issue(
        &mut self,
        creds: &AtlassianCredentials,
        source_id: &str,
        issue_key: &str,
    ) -> Result<()> {
        info!("Syncing single JIRA issue: {}", issue_key);

        let fields = vec![
            "summary",
            "description",
            "issuetype",
            "status",
            "priority",
            "assignee",
            "reporter",
            "creator",
            "project",
            "created",
            "updated",
            "labels",
            "comment",
            "components",
        ];

        let issue = self
            .client
            .get_jira_issue_by_key(creds, issue_key, &fields)
            .await?;

        let content = issue.to_document_content();
        if content.trim().is_empty() {
            warn!("Issue {} has no content, skipping", issue_key);
            return Ok(());
        }

        // TODO: Add proper sync_run_id when sync runs are implemented for Atlassian
        let placeholder_sync_run_id = shared::utils::generate_ulid();
        let event = issue.to_connector_event(
            placeholder_sync_run_id,
            source_id.to_string(),
            &creds.base_url,
        );
        self.event_queue.enqueue(source_id, &event).await?;

        info!("Successfully queued issue: {}", issue.fields.summary);
        Ok(())
    }

    pub async fn delete_issue(
        &self,
        source_id: &str,
        project_key: &str,
        issue_key: &str,
    ) -> Result<()> {
        info!("Deleting JIRA issue: {}", issue_key);

        let document_id = format!("jira_issue_{}_{}", project_key, issue_key);
        // TODO: Add proper sync_run_id when sync runs are implemented for Atlassian
        let placeholder_sync_run_id = shared::utils::generate_ulid();
        let event = shared::models::ConnectorEvent::DocumentDeleted {
            sync_run_id: placeholder_sync_run_id,
            source_id: source_id.to_string(),
            document_id,
        };

        self.event_queue.enqueue(source_id, &event).await?;
        info!("Successfully queued deletion for issue: {}", issue_key);
        Ok(())
    }

    pub async fn sync_issues_by_jql(
        &mut self,
        creds: &AtlassianCredentials,
        source_id: &str,
        jql: &str,
        max_results: Option<u32>,
    ) -> Result<u32> {
        info!("Syncing JIRA issues by JQL: {}", jql);

        let mut total_issues = 0;
        let mut start_at = 0;
        const PAGE_SIZE: u32 = 50;
        let max_results = max_results.unwrap_or(u32::MAX);

        let fields = vec![
            "summary",
            "description",
            "issuetype",
            "status",
            "priority",
            "assignee",
            "reporter",
            "creator",
            "project",
            "created",
            "updated",
            "labels",
            "comment",
            "components",
        ];

        loop {
            if total_issues >= max_results {
                break;
            }

            let page_size = std::cmp::min(PAGE_SIZE, max_results - total_issues);
            let response = self
                .client
                .get_jira_issues(creds, jql, page_size, start_at, &fields)
                .await?;

            if response.issues.is_empty() {
                break;
            }

            let issues_count = response.issues.len();
            let events = self.process_issues(response.issues, source_id, &creds.base_url)?;
            self.queue_events(events).await?;

            total_issues += issues_count as u32;
            start_at += page_size;

            debug!(
                "Processed {} issues from JQL query, total: {}",
                issues_count, total_issues
            );

            // Check if we've reached the end
            if issues_count < page_size as usize {
                break;
            }
        }

        info!(
            "Completed JQL-based JIRA sync. Issues processed: {}",
            total_issues
        );
        Ok(total_issues)
    }

    pub async fn sync_issues_by_status(
        &mut self,
        creds: &AtlassianCredentials,
        source_id: &str,
        status: &str,
        project_key: Option<&str>,
    ) -> Result<u32> {
        let mut jql = format!("status = '{}'", status);

        if let Some(project) = project_key {
            jql = format!("project = {} AND {}", project, jql);
        }

        self.sync_issues_by_jql(creds, source_id, &jql, None).await
    }

    pub async fn sync_issues_assigned_to(
        &mut self,
        creds: &AtlassianCredentials,
        source_id: &str,
        assignee: &str,
        project_key: Option<&str>,
    ) -> Result<u32> {
        let mut jql = format!("assignee = '{}'", assignee);

        if let Some(project) = project_key {
            jql = format!("project = {} AND {}", project, jql);
        }

        self.sync_issues_by_jql(creds, source_id, &jql, None).await
    }

    pub fn get_rate_limit_info(&self) -> String {
        let rate_limit = self.client.get_rate_limit_info();
        if let Some(remaining) = rate_limit.requests_remaining {
            format!("Requests remaining: {}", remaining)
        } else {
            "Rate limit info not available".to_string()
        }
    }
}
