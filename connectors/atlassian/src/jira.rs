use anyhow::{anyhow, Result};
use chrono::{DateTime, Duration, Utc};
use dashmap::DashMap;
use omni_connector_sdk::{DocumentPermissions, SdkClient, SyncContext};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use crate::auth::AtlassianCredentials;
use crate::client::AtlassianApi;
use crate::models::JiraIssue;
use crate::user_resolver::UserResolver;

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
    "security",
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
    client: Arc<dyn AtlassianApi>,
    sdk_client: SdkClient,
    user_resolver: Arc<UserResolver>,
    cached_custom_fields: RwLock<Option<(Vec<String>, DateTime<Utc>)>>,
    project_permissions_cache: DashMap<String, DocumentPermissions>,
    /// groupId → display_name for groups encountered in project role actors
    /// during this sync. Drained at end of sync by SyncManager so it can
    /// emit one GroupMembershipSync event per encountered group.
    encountered_groups: DashMap<String, Option<String>>,
    /// Per-sync cache of `securityLevelId → DocumentPermissions` populated as
    /// projects are encountered. When an issue has `fields.security` set, its
    /// effective permissions become the level's perms (which are narrower than
    /// the project's). One entry per security level the connector ever sees.
    security_level_perms: DashMap<String, DocumentPermissions>,
    /// Tracks which projects have already had their security scheme resolved
    /// and folded into `security_level_perms`, so we don't re-fetch on every
    /// issue.
    security_resolved_projects: DashMap<String, ()>,
}

const CUSTOM_FIELDS_CACHE_TTL_DAYS: i64 = 1;

impl JiraProcessor {
    pub fn new(client: Arc<dyn AtlassianApi>, sdk_client: SdkClient) -> Self {
        let resolver = Arc::new(UserResolver::new(client.clone(), Arc::new(HashMap::new())));
        Self::with_resolver(client, sdk_client, resolver)
    }

    pub fn with_resolver(
        client: Arc<dyn AtlassianApi>,
        sdk_client: SdkClient,
        user_resolver: Arc<UserResolver>,
    ) -> Self {
        Self {
            client,
            sdk_client,
            user_resolver,
            cached_custom_fields: RwLock::new(None),
            project_permissions_cache: DashMap::new(),
            encountered_groups: DashMap::new(),
            security_level_perms: DashMap::new(),
            security_resolved_projects: DashMap::new(),
        }
    }

    /// Drain the set of groupIds encountered in project permissions during the
    /// sync so the SyncManager can fetch their members and emit one
    /// GroupMembershipSync event per group.
    pub fn drain_encountered_groups(&self) -> HashMap<String, Option<String>> {
        self.encountered_groups
            .iter()
            .map(|entry| (entry.key().clone(), entry.value().clone()))
            .collect()
    }

    /// Resolve a project's issue security scheme (if any) and populate the
    /// per-sync `security_level_perms` cache for every level in the scheme.
    /// Idempotent and short-circuits per project.
    async fn ensure_security_levels_for_project(
        &self,
        creds: &AtlassianCredentials,
        project_key: &str,
    ) {
        if self.security_resolved_projects.contains_key(project_key) {
            return;
        }
        // Mark resolved up-front so concurrent issues for the same project
        // don't race; if the fetch fails we still avoid retrying per-issue.
        self.security_resolved_projects
            .insert(project_key.to_string(), ());

        let scheme = match self
            .client
            .get_project_issue_security_scheme(creds, project_key)
            .await
        {
            Ok(Some(s)) => s,
            Ok(None) => return,
            Err(e) => {
                warn!(
                    "Failed to fetch issue security scheme for project {}: {}",
                    project_key, e
                );
                return;
            }
        };

        let scheme_detail = match self
            .client
            .get_issue_security_scheme(creds, &scheme.id)
            .await
        {
            Ok(s) => s,
            Err(e) => {
                warn!(
                    "Failed to fetch security scheme {} detail for project {}: {}",
                    scheme.id, project_key, e
                );
                return;
            }
        };

        for level in &scheme_detail.levels {
            // Skip if some other project already populated this level.
            if self.security_level_perms.contains_key(&level.id) {
                continue;
            }
            let members = match self
                .client
                .get_issue_security_level_members(creds, &scheme.id, &level.id)
                .await
            {
                Ok(m) => m,
                Err(e) => {
                    warn!(
                        "Failed to fetch members for security scheme {} level {}: {}",
                        scheme.id, level.id, e
                    );
                    continue;
                }
            };

            let mut user_account_ids = Vec::new();
            let mut group_ids = Vec::new();
            for member in members {
                match member.holder.holder_type.as_str() {
                    "user" => {
                        if let Some(account_id) = member.holder.parameter {
                            user_account_ids.push(account_id);
                        }
                    }
                    "group" | "groupCustomField" => {
                        if let Some(group_id) = member.holder.parameter {
                            group_ids.push(group_id.clone());
                            self.encountered_groups.entry(group_id).or_insert(None);
                        }
                    }
                    other => {
                        warn!(
                            "Unhandled security level holder type '{}' on scheme {} level {}",
                            other, scheme.id, level.id
                        );
                    }
                }
            }

            // Strip the service account from restriction lists — it is not
            // a meaningful permission grant for end-user authz.
            let sa_account_id = creds.sa_account_id.as_deref();
            user_account_ids.retain(|id| Some(id.as_str()) != sa_account_id);

            user_account_ids.sort();
            user_account_ids.dedup();
            group_ids.sort();
            group_ids.dedup();

            let mut user_emails = Vec::new();
            if !user_account_ids.is_empty() {
                match self
                    .user_resolver
                    .resolve_emails(creds, &user_account_ids)
                    .await
                {
                    Ok(pairs) => user_emails.extend(pairs.into_iter().map(|(_, e)| e)),
                    Err(e) => warn!(
                        "Failed to resolve emails for security scheme {} level {}: {}",
                        scheme.id, level.id, e
                    ),
                }
            }

            if !user_account_ids.is_empty() && user_emails.is_empty() {
                warn!(
                    "Security scheme {} level {} has individual user restrictions \
                     but none could be resolved to emails. The level will be treated as private. \
                     Configure an org-admin API key to resolve user emails.",
                    scheme.id, level.id
                );
            }

            user_emails.sort();
            user_emails.dedup();

            // FIXME(groups-key-rename): we're storing an Atlassian groupId in the
            // `groups.email` column. Rename that column (and the GroupMembershipSync
            // `group_email` field) to a generic identifier so non-email-keyed group
            // systems are supported first-class.
            self.security_level_perms.insert(
                level.id.clone(),
                DocumentPermissions {
                    public: false,
                    users: user_emails,
                    groups: group_ids,
                },
            );
        }
    }

    async fn get_project_permissions(
        &self,
        creds: &AtlassianCredentials,
        project_key: &str,
    ) -> DocumentPermissions {
        if let Some(cached) = self.project_permissions_cache.get(project_key) {
            return cached.clone();
        }

        let perms = match self.fetch_project_permissions(creds, project_key).await {
            Ok(p) => p,
            Err(e) => {
                warn!(
                    "Failed to fetch permissions for project {}, defaulting to public: {}",
                    project_key, e
                );
                DocumentPermissions {
                    public: true,
                    users: vec![],
                    groups: vec![],
                }
            }
        };

        self.project_permissions_cache
            .insert(project_key.to_string(), perms.clone());
        perms
    }

    async fn fetch_project_permissions(
        &self,
        creds: &AtlassianCredentials,
        project_key: &str,
    ) -> Result<DocumentPermissions> {
        let role_response = self
            .client
            .get_jira_project_roles(creds, project_key)
            .await?;

        let mut user_account_ids = Vec::new();
        let mut group_ids: Vec<String> = Vec::new();
        let mut public = false;

        // Fetch actors for each role
        for (_role_name, role_url) in &role_response.roles {
            // Extract role ID from URL (e.g., ".../role/10002")
            let role_id = role_url.rsplit('/').next().unwrap_or_default();

            if role_id.is_empty() {
                continue;
            }

            match self
                .client
                .get_jira_project_role_actors(creds, project_key, role_id)
                .await
            {
                Ok(role_actors) => {
                    for actor in &role_actors.actors {
                        match actor.actor_type.as_str() {
                            "atlassian-user-role-actor" => {
                                if let Some(user) = &actor.actor_user {
                                    user_account_ids.push(user.account_id.clone());
                                }
                            }
                            "atlassian-group-role-actor" => {
                                if let Some(group) = &actor.actor_group {
                                    match &group.group_id {
                                        Some(gid) => {
                                            group_ids.push(gid.clone());
                                            self.encountered_groups
                                                .entry(gid.clone())
                                                .or_insert_with(|| {
                                                    Some(group.display_name.clone())
                                                });
                                        }
                                        None => {
                                            warn!(
                                                "Skipping group actor in project {} role {}: missing groupId (group name: {})",
                                                project_key, role_id, group.name
                                            );
                                        }
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }
                Err(e) => {
                    warn!(
                        "Failed to fetch actors for role {} in project {}: {}",
                        role_id, project_key, e
                    );
                }
            }
        }

        // Walk the project's permission scheme for BROWSE_PROJECTS grants
        // whose holder is NOT a projectRole — those (user/group/anyone/
        // applicationRole/projectLead) are missed by the role-actor traversal
        // above. Failure here downgrades to "role-actor-only" perms rather
        // than aborting the project sync.
        match self
            .client
            .get_project_permission_scheme(creds, project_key)
            .await
        {
            Ok(scheme) => {
                for grant in scheme.permissions {
                    if grant.permission != "BROWSE_PROJECTS" {
                        continue;
                    }
                    match grant.holder.holder_type.as_str() {
                        // projectRole grants are covered by the role-actor
                        // walker above; skip to avoid double-counting.
                        "projectRole" => {}
                        "user" => match grant.holder.identifier() {
                            Some(account_id) => user_account_ids.push(account_id.to_string()),
                            None => warn!(
                                "BROWSE_PROJECTS user grant in project {} has no identifier",
                                project_key
                            ),
                        },
                        "group" | "groupCustomField" => match grant.holder.identifier() {
                            Some(group_id) => {
                                group_ids.push(group_id.to_string());
                                self.encountered_groups
                                    .entry(group_id.to_string())
                                    .or_insert(None);
                            }
                            None => warn!(
                                "BROWSE_PROJECTS group grant in project {} has no identifier",
                                project_key
                            ),
                        },
                        "anyone" => {
                            public = true;
                        }
                        "applicationRole" => {
                            // An applicationRole grant means "any user with
                            // access to this Atlassian application." For
                            // BROWSE_PROJECTS this effectively grants
                            // company-wide read. Treat as public.
                            public = true;
                        }
                        "projectLead" => {
                            // The lead's accountId is on the project metadata,
                            // not in the grant. Skip with a warn — we'd need
                            // to plumb the project response in to resolve it
                            // and the lead is typically already covered by a
                            // role grant anyway.
                            warn!(
                                "BROWSE_PROJECTS projectLead grant in project {} not yet resolved",
                                project_key
                            );
                        }
                        other => {
                            // assignee / reporter / currentUser / userCustomField:
                            // dynamic, can't statically resolve.
                            warn!(
                                "Unsupported BROWSE_PROJECTS holder type '{}' in project {}",
                                other, project_key
                            );
                        }
                    }
                }
            }
            Err(e) => {
                warn!(
                    "Failed to fetch permission scheme for project {} (continuing with role actors only): {}",
                    project_key, e
                );
            }
        }

        // Strip the service account from permission lists — it is not
        // a meaningful permission grant for end-user authz.
        let sa_account_id = creds.sa_account_id.as_deref();
        user_account_ids.retain(|id| Some(id.as_str()) != sa_account_id);

        // Resolve accountIds to emails
        let mut user_emails = Vec::new();
        if !user_account_ids.is_empty() {
            user_account_ids.sort();
            user_account_ids.dedup();
            match self
                .user_resolver
                .resolve_emails(creds, &user_account_ids)
                .await
            {
                Ok(id_email_pairs) => {
                    user_emails.extend(id_email_pairs.into_iter().map(|(_, email)| email));
                }
                Err(e) => {
                    warn!(
                        "Failed to resolve user emails for project {}: {}",
                        project_key, e
                    );
                }
            }
        }

        if !user_account_ids.is_empty() && user_emails.is_empty() {
            warn!(
                "Project {} has individual user permissions but none could be resolved to emails. \
                 The project will be treated as private in Omni. \
                 Configure an org-admin API key to resolve user emails.",
                project_key
            );
        }

        user_emails.sort();
        user_emails.dedup();
        group_ids.sort();
        group_ids.dedup();

        // FIXME(groups-key-rename): we're storing an Atlassian groupId in the
        // `groups.email` column. Rename that column (and the GroupMembershipSync
        // `group_email` field) to a generic identifier so non-email-keyed group
        // systems are supported first-class.
        Ok(DocumentPermissions {
            public,
            users: user_emails,
            groups: group_ids,
        })
    }

    async fn get_custom_field_ids(&self, creds: &AtlassianCredentials) -> Vec<String> {
        {
            let cache = self.cached_custom_fields.read().await;
            if let Some((ids, fetched_at)) = cache.as_ref() {
                if Utc::now() - *fetched_at < Duration::days(CUSTOM_FIELDS_CACHE_TTL_DAYS) {
                    return ids.clone();
                }
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
                *self.cached_custom_fields.write().await = Some((custom.clone(), Utc::now()));
                custom
            }
            Err(e) => {
                warn!("Failed to fetch custom fields, using defaults only: {}", e);
                vec![]
            }
        }
    }

    pub async fn sync_all_projects(
        &self,
        creds: &AtlassianCredentials,
        source_id: &str,
        sync_run_id: &str,
        ctx: &SyncContext,
        project_filters: &Option<Vec<String>>,
    ) -> Result<u32> {
        info!(
            "Starting JIRA projects sync for source: {} (sync_run_id: {})",
            source_id, sync_run_id
        );

        let custom_field_ids = self.get_custom_field_ids(creds).await;
        let all_projects = self.get_accessible_projects(creds).await?;
        let projects: Vec<serde_json::Value> = match project_filters {
            Some(filters) => {
                let filtered: Vec<serde_json::Value> = all_projects
                    .into_iter()
                    .filter(|p| {
                        p.get("key")
                            .and_then(|k| k.as_str())
                            .map(|k| filters.iter().any(|f| f.eq_ignore_ascii_case(k)))
                            .unwrap_or(false)
                    })
                    .collect();
                info!(
                    "Filtered to {} projects (from {} accessible)",
                    filtered.len(),
                    filters.len()
                );
                filtered
            }
            None => all_projects,
        };
        let mut total_issues_processed = 0;

        for project in projects {
            if ctx.is_cancelled() {
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
                    ctx,
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
        &self,
        creds: &AtlassianCredentials,
        source_id: &str,
        since: DateTime<Utc>,
        project_filters: Option<&Vec<String>>,
        sync_run_id: &str,
        ctx: &SyncContext,
    ) -> Result<u32> {
        info!(
            "Starting incremental JIRA sync for source: {} since {}{} (sync_run_id: {})",
            source_id,
            since.format("%Y-%m-%d %H:%M:%S"),
            project_filters.map_or(String::new(), |f| format!(" (projects: {:?})", f)),
            sync_run_id
        );

        let custom_field_ids = self.get_custom_field_ids(creds).await;

        let since_str = since.format("%Y-%m-%d %H:%M").to_string();
        let mut jql = format!("updated >= '{}'", since_str);
        if let Some(filters) = project_filters {
            if !filters.is_empty() {
                let projects_str = filters.join(", ");
                jql = format!("project IN ({}) AND {}", projects_str, jql);
            }
        }

        let fields = build_fields(Some(&custom_field_ids));
        let mut total_issues = 0;
        let mut next_page_token: Option<String> = None;
        const PAGE_SIZE: u32 = 50;

        loop {
            if ctx.is_cancelled() {
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
                .process_issues(
                    response.issues,
                    source_id,
                    &creds.site_base(),
                    sync_run_id,
                    creds,
                )
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
        &self,
        creds: &AtlassianCredentials,
        source_id: &str,
        project_key: &str,
        sync_run_id: &str,
        ctx: &SyncContext,
        custom_fields: Option<&[String]>,
    ) -> Result<u32> {
        let mut total_issues = 0;
        let mut next_page_token: Option<String> = None;
        const PAGE_SIZE: u32 = 50;

        let jql = format!("project = {}", project_key);
        let fields = build_fields(custom_fields);

        loop {
            if ctx.is_cancelled() {
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
                .process_issues(
                    response.issues,
                    source_id,
                    &creds.site_base(),
                    sync_run_id,
                    creds,
                )
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
        &self,
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
        creds: &AtlassianCredentials,
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

            let project_key = issue.fields.project.key.clone();
            let project_perms = self.get_project_permissions(creds, &project_key).await;
            self.ensure_security_levels_for_project(creds, &project_key)
                .await;
            let permissions = match &issue.fields.security {
                Some(level) => match self.security_level_perms.get(&level.id) {
                    Some(level_perms) => level_perms.clone(),
                    None => {
                        // Scheme/level fetch failed earlier; we know the issue
                        // is restricted but can't enumerate the holders. Be
                        // safe: emit empty perms so nobody sees it rather than
                        // falling back to project perms (which would over-grant).
                        warn!(
                            "Issue {} has security level {} but its members could not be resolved; emitting empty perms",
                            issue.key, level.id
                        );
                        DocumentPermissions {
                            public: false,
                            users: vec![],
                            groups: vec![],
                        }
                    }
                },
                None => project_perms,
            };

            let event = issue.to_connector_event(
                sync_run_id.to_string(),
                source_id.to_string(),
                base_url,
                content_id,
                permissions,
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
}
