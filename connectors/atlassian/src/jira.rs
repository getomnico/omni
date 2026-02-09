use anyhow::{anyhow, Result};
use chrono::{DateTime, Duration, Utc};
use shared::models::{ConnectorEvent, SyncType};
use std::sync::atomic::{AtomicBool, Ordering};
use tracing::{debug, error, info, warn};

use crate::auth::AtlassianCredentials;
use crate::client::AtlassianClient;
use crate::models::JiraIssue;
use shared::SdkClient;

const DEFAULT_JIRA_FIELDS: &[&str] = &[
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

fn build_fields(custom_fields: Option<&[String]>) -> Vec<String> {
    let mut fields: Vec<String> = DEFAULT_JIRA_FIELDS.iter().map(|s| s.to_string()).collect();
    if let Some(cf) = custom_fields {
        let new_fields: Vec<String> = cf.iter().filter(|f| !fields.contains(f)).cloned().collect();
        fields.extend(new_fields);
    }
    fields
}

pub struct JiraProcessor {
    client: AtlassianClient,
    sdk_client: SdkClient,
    cached_custom_fields: Option<(Vec<String>, DateTime<Utc>)>,
}

const CUSTOM_FIELDS_CACHE_TTL_DAYS: i64 = 1;

impl JiraProcessor {
    pub fn new(sdk_client: SdkClient) -> Self {
        Self {
            client: AtlassianClient::new(),
            sdk_client,
            cached_custom_fields: None,
        }
    }

    async fn get_custom_field_ids(&mut self, creds: &AtlassianCredentials) -> Vec<String> {
        if let Some((ref ids, fetched_at)) = self.cached_custom_fields {
            if Utc::now() - fetched_at < Duration::days(CUSTOM_FIELDS_CACHE_TTL_DAYS) {
                return ids.clone();
            }
        }

        match self.client.get_jira_fields(creds).await {
            Ok(fields) => {
                let custom: Vec<String> = fields
                    .into_iter()
                    .filter(|f| f.custom)
                    .map(|f| f.id)
                    .collect();
                debug!("Discovered {} custom fields", custom.len());
                self.cached_custom_fields = Some((custom.clone(), Utc::now()));
                custom
            }
            Err(e) => {
                warn!("Failed to fetch custom fields, using defaults only: {}", e);
                vec![]
            }
        }
    }

    pub async fn sync_all_projects(
        &mut self,
        creds: &AtlassianCredentials,
        source_id: &str,
        sync_run_id: &str,
        cancelled: &AtomicBool,
    ) -> Result<u32> {
        info!(
            "Starting JIRA projects sync for source: {} (sync_run_id: {})",
            source_id, sync_run_id
        );

        let custom_field_ids = self.get_custom_field_ids(creds).await;
        let projects = self.get_accessible_projects(creds).await?;
        let mut total_issues_processed = 0;

        for project in projects {
            if cancelled.load(Ordering::SeqCst) {
                info!(
                    "JIRA sync {} cancelled, stopping early after {} issues",
                    sync_run_id, total_issues_processed
                );
                return Ok(total_issues_processed);
            }

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
                .sync_project_issues(
                    creds,
                    source_id,
                    project_key,
                    sync_run_id,
                    cancelled,
                    Some(&custom_field_ids),
                )
                .await
            {
                Ok(issues_count) => {
                    total_issues_processed += issues_count;
                    info!(
                        "Synced {} issues from project: {}",
                        issues_count, project_key
                    );
                    // Update scanned count via SDK
                    if let Err(e) = self
                        .sdk_client
                        .increment_scanned(sync_run_id, issues_count as i32)
                        .await
                    {
                        error!("Failed to increment scanned count: {}", e);
                    }
                }
                Err(e) => {
                    error!("Failed to sync project {}: {}", project_key, e);
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
        sync_run_id: &str,
        cancelled: &AtomicBool,
    ) -> Result<u32> {
        info!(
            "Starting incremental JIRA sync for source: {} since {}{} (sync_run_id: {})",
            source_id,
            since.format("%Y-%m-%d %H:%M:%S"),
            project_key.map_or(String::new(), |p| format!(" (project: {})", p)),
            sync_run_id
        );

        let custom_field_ids = self.get_custom_field_ids(creds).await;

        let since_str = since.format("%Y-%m-%d %H:%M").to_string();
        let mut jql = format!("updated >= '{}'", since_str);
        if let Some(project) = project_key {
            jql = format!("project = {} AND {}", project, jql);
        }

        let fields = build_fields(Some(&custom_field_ids));
        let mut total_issues = 0;
        let mut next_page_token: Option<String> = None;
        const PAGE_SIZE: u32 = 50;

        loop {
            if cancelled.load(Ordering::SeqCst) {
                info!(
                    "JIRA incremental sync {} cancelled, stopping after {} issues",
                    sync_run_id, total_issues
                );
                return Ok(total_issues);
            }

            let response = self
                .client
                .get_jira_issues(creds, &jql, PAGE_SIZE, next_page_token.as_deref(), &fields)
                .await?;

            if response.issues.is_empty() {
                break;
            }

            let issues_count = response.issues.len();
            let count = self
                .process_issues(response.issues, source_id, &creds.base_url, sync_run_id)
                .await?;

            total_issues += count;

            debug!(
                "Processed {} issues, total so far: {}",
                issues_count, total_issues
            );

            if response.is_last || response.next_page_token.is_none() {
                break;
            }
            next_page_token = response.next_page_token;
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
        sync_run_id: &str,
        cancelled: &AtomicBool,
        custom_fields: Option<&[String]>,
    ) -> Result<u32> {
        let mut total_issues = 0;
        let mut next_page_token: Option<String> = None;
        const PAGE_SIZE: u32 = 50;

        let jql = format!("project = {}", project_key);
        let fields = build_fields(custom_fields);

        loop {
            if cancelled.load(Ordering::SeqCst) {
                info!(
                    "JIRA project {} sync cancelled, stopping after {} issues",
                    project_key, total_issues
                );
                return Ok(total_issues);
            }

            let response = self
                .client
                .get_jira_issues(creds, &jql, PAGE_SIZE, next_page_token.as_deref(), &fields)
                .await?;

            if response.issues.is_empty() {
                break;
            }

            let issues_count = response.issues.len();
            let count = self
                .process_issues(response.issues, source_id, &creds.base_url, sync_run_id)
                .await?;

            total_issues += count;

            debug!(
                "Processed {} issues from project {}, total: {}",
                issues_count, project_key, total_issues
            );

            if response.is_last || response.next_page_token.is_none() {
                break;
            }
            next_page_token = response.next_page_token;
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

        let custom_field_ids = self.get_custom_field_ids(creds).await;

        // Create sync run via SDK
        let sync_run_id = self
            .sdk_client
            .create_sync_run(source_id, SyncType::Incremental)
            .await
            .map_err(|e| anyhow!("Failed to create sync run via SDK: {}", e))?;

        let fields = build_fields(Some(&custom_field_ids));

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
                .store_content(&sync_run_id, &content)
                .await
                .map_err(|e| {
                    anyhow!(
                        "Failed to store content via SDK for Jira issue {}: {}",
                        issue.key,
                        e
                    )
                })?;

            let event = issue.to_connector_event(
                sync_run_id.clone(),
                source_id.to_string(),
                &creds.base_url,
                content_id,
            );
            self.sdk_client
                .emit_event(&sync_run_id, source_id, event)
                .await?;

            info!("Successfully queued issue: {}", issue.fields.summary);
            Ok(())
        }
        .await;

        // Mark sync as completed or failed via SDK
        match &result {
            Ok(_) => {
                self.sdk_client.complete(&sync_run_id, 1, 1, None).await?;
            }
            Err(e) => {
                self.sdk_client.fail(&sync_run_id, &e.to_string()).await?;
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

        let custom_field_ids = self.get_custom_field_ids(creds).await;

        // Create sync run via SDK
        let sync_run_id = self
            .sdk_client
            .create_sync_run(source_id, SyncType::Incremental)
            .await
            .map_err(|e| anyhow!("Failed to create sync run via SDK: {}", e))?;

        let mut total_issues = 0;
        let mut next_page_token: Option<String> = None;
        const PAGE_SIZE: u32 = 50;
        let max_results = max_results.unwrap_or(u32::MAX);

        let fields = build_fields(Some(&custom_field_ids));

        let result: Result<u32> = async {
            loop {
                if total_issues >= max_results {
                    break;
                }

                let page_size = std::cmp::min(PAGE_SIZE, max_results - total_issues);
                let response = self
                    .client
                    .get_jira_issues(creds, jql, page_size, next_page_token.as_deref(), &fields)
                    .await?;

                if response.issues.is_empty() {
                    break;
                }

                let issues_count = response.issues.len();
                let count = self
                    .process_issues(response.issues, source_id, &creds.base_url, &sync_run_id)
                    .await?;

                total_issues += count;

                debug!(
                    "Processed {} issues from JQL query, total: {}",
                    issues_count, total_issues
                );

                if response.is_last || response.next_page_token.is_none() {
                    break;
                }
                next_page_token = response.next_page_token;
            }

            info!(
                "Completed JQL-based JIRA sync. Issues processed: {}",
                total_issues
            );
            Ok(total_issues)
        }
        .await;

        // Mark sync as completed or failed via SDK
        match &result {
            Ok(count) => {
                self.sdk_client
                    .complete(&sync_run_id, *count as i32, *count as i32, None)
                    .await?;
            }
            Err(e) => {
                self.sdk_client.fail(&sync_run_id, &e.to_string()).await?;
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
