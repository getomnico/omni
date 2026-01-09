use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use shared::db::repositories::SyncRunRepository;
use shared::models::{ConnectorEvent, SyncType};
use tracing::{debug, error, info, warn};

use crate::auth::AtlassianCredentials;
use crate::client::AtlassianClient;
use crate::models::JiraIssue;
use shared::SdkClient;

pub struct JiraProcessor {
    client: AtlassianClient,
    sdk_client: SdkClient,
    sync_run_repo: SyncRunRepository,
}

impl JiraProcessor {
    pub fn new(sdk_client: SdkClient, sync_run_repo: SyncRunRepository) -> Self {
        Self {
            client: AtlassianClient::new(),
            sdk_client,
            sync_run_repo,
        }
    }

    pub async fn sync_all_projects(
        &mut self,
        creds: &AtlassianCredentials,
        source_id: &str,
    ) -> Result<u32> {
        info!("Starting JIRA projects sync for source: {}", source_id);

        let sync_run = self.sync_run_repo.create(source_id, SyncType::Full).await?;

        let projects = match self.get_accessible_projects(creds).await {
            Ok(p) => p,
            Err(e) => {
                self.sync_run_repo
                    .mark_failed(&sync_run.id, &e.to_string())
                    .await?;
                return Err(e);
            }
        };

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
                .sync_project_issues(creds, source_id, project_key, &sync_run.id)
                .await
            {
                Ok(issues_count) => {
                    total_issues_processed += issues_count;
                    info!(
                        "Synced {} issues from project: {}",
                        issues_count, project_key
                    );
                    // Update scanned count via SDK
                    if let Err(e) = self.sdk_client.increment_scanned(&sync_run.id).await {
                        error!("Failed to increment scanned count: {}", e);
                    }
                }
                Err(e) => {
                    error!("Failed to sync project {}: {}", project_key, e);
                }
            }
        }

        self.sync_run_repo
            .mark_completed(
                &sync_run.id,
                total_issues_processed as i32,
                total_issues_processed as i32,
            )
            .await?;

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

        let sync_run = self
            .sync_run_repo
            .create(source_id, SyncType::Incremental)
            .await?;

        let since_str = since.format("%Y-%m-%d %H:%M").to_string();
        let mut total_issues = 0;
        let mut start_at = 0;
        const PAGE_SIZE: u32 = 50;

        let result: Result<u32> = async {
            loop {
                let response = self
                    .client
                    .get_jira_issues_updated_since(
                        creds,
                        &since_str,
                        project_key,
                        PAGE_SIZE,
                        start_at,
                    )
                    .await?;

                if response.issues.is_empty() {
                    break;
                }

                let issues_count = response.issues.len();
                let count = self
                    .process_issues(response.issues, source_id, &creds.base_url, &sync_run.id)
                    .await?;

                total_issues += count;
                start_at += PAGE_SIZE;

                debug!(
                    "Processed {} issues, total so far: {}",
                    issues_count, total_issues
                );

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
        .await;

        match &result {
            Ok(count) => {
                self.sync_run_repo
                    .mark_completed(&sync_run.id, *count as i32, *count as i32)
                    .await?
            }
            Err(e) => {
                self.sync_run_repo
                    .mark_failed(&sync_run.id, &e.to_string())
                    .await?
            }
        }

        result
    }

    async fn sync_project_issues(
        &mut self,
        creds: &AtlassianCredentials,
        source_id: &str,
        project_key: &str,
        sync_run_id: &str,
    ) -> Result<u32> {
        let mut total_issues = 0;
        let mut start_at = 0;
        const PAGE_SIZE: u32 = 50;

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
            let count = self
                .process_issues(response.issues, source_id, &creds.base_url, sync_run_id)
                .await?;

            total_issues += count;
            start_at += PAGE_SIZE;

            debug!(
                "Processed {} issues from project {}, total: {}",
                issues_count, project_key, total_issues
            );

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

    async fn process_issues(
        &self,
        issues: Vec<JiraIssue>,
        source_id: &str,
        base_url: &str,
        sync_run_id: &str,
    ) -> Result<u32> {
        let mut count = 0;

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

            // Store content via SDK
            let content_id = match self.sdk_client.store_content(sync_run_id, &content).await {
                Ok(id) => id,
                Err(e) => {
                    error!(
                        "Failed to store content via SDK for Jira issue {}: {}",
                        issue.key, e
                    );
                    continue;
                }
            };

            let event = issue.to_connector_event(
                sync_run_id.to_string(),
                source_id.to_string(),
                base_url,
                content_id,
            );

            // Emit event via SDK
            if let Err(e) = self
                .sdk_client
                .emit_event(sync_run_id, source_id, event)
                .await
            {
                error!("Failed to emit event for JIRA issue {}: {}", issue.key, e);
                continue;
            }

            count += 1;
        }

        Ok(count)
    }

    pub async fn sync_single_issue(
        &mut self,
        creds: &AtlassianCredentials,
        source_id: &str,
        issue_key: &str,
    ) -> Result<()> {
        info!("Syncing single JIRA issue: {}", issue_key);

        let sync_run = self
            .sync_run_repo
            .create(source_id, SyncType::Incremental)
            .await?;

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

        let result: Result<()> = async {
            let issue = self
                .client
                .get_jira_issue_by_key(creds, issue_key, &fields)
                .await?;

            let content = issue.to_document_content();
            if content.trim().is_empty() {
                warn!("Issue {} has no content, skipping", issue_key);
                return Ok(());
            }

            let content_id = self
                .sdk_client
                .store_content(&sync_run.id, &content)
                .await
                .map_err(|e| {
                    anyhow!(
                        "Failed to store content via SDK for Jira issue {}: {}",
                        issue.key,
                        e
                    )
                })?;

            let event = issue.to_connector_event(
                sync_run.id.clone(),
                source_id.to_string(),
                &creds.base_url,
                content_id,
            );
            self.sdk_client
                .emit_event(&sync_run.id, source_id, event)
                .await?;

            info!("Successfully queued issue: {}", issue.fields.summary);
            Ok(())
        }
        .await;

        match &result {
            Ok(_) => {
                self.sync_run_repo
                    .mark_completed(&sync_run.id, 1, 1)
                    .await?
            }
            Err(e) => {
                self.sync_run_repo
                    .mark_failed(&sync_run.id, &e.to_string())
                    .await?
            }
        }

        result
    }

    pub async fn delete_issue(
        &self,
        source_id: &str,
        sync_run_id: &str,
        project_key: &str,
        issue_key: &str,
    ) -> Result<()> {
        info!("Deleting JIRA issue: {}", issue_key);

        let document_id = format!("jira_issue_{}_{}", project_key, issue_key);
        let event = ConnectorEvent::DocumentDeleted {
            sync_run_id: sync_run_id.to_string(),
            source_id: source_id.to_string(),
            document_id,
        };

        self.sdk_client
            .emit_event(sync_run_id, source_id, event)
            .await?;
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

        let sync_run = self
            .sync_run_repo
            .create(source_id, SyncType::Incremental)
            .await?;

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

        let result: Result<u32> = async {
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
                let count = self
                    .process_issues(response.issues, source_id, &creds.base_url, &sync_run.id)
                    .await?;

                total_issues += count;
                start_at += page_size;

                debug!(
                    "Processed {} issues from JQL query, total: {}",
                    issues_count, total_issues
                );

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
        .await;

        match &result {
            Ok(count) => {
                self.sync_run_repo
                    .mark_completed(&sync_run.id, *count as i32, *count as i32)
                    .await?
            }
            Err(e) => {
                self.sync_run_repo
                    .mark_failed(&sync_run.id, &e.to_string())
                    .await?
            }
        }

        result
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
}
