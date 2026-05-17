use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use omni_connector_sdk::{ConfluenceSourceConfig, JiraSourceConfig, ServiceProvider};
use omni_connector_sdk::{
    ConnectorEvent, SdkClient, ServiceCredential, Source, SourceType, SyncContext, SyncType,
};
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, info, warn};

use crate::auth::{AtlassianCredentials, AuthManager};
use crate::client::{AtlassianApi, OrgGroupInfo};
use crate::confluence::ConfluenceProcessor;
use crate::jira::JiraProcessor;
use crate::models::{AtlassianConnectorState, AtlassianWebhookEvent};
use crate::user_resolver::UserResolver;

pub struct SyncManager {
    pub sdk_client: SdkClient,
    auth_manager: AuthManager,
    client: Arc<dyn AtlassianApi>,
    webhook_url: Option<String>,
}

impl SyncManager {
    pub fn new(sdk_client: SdkClient, webhook_url: Option<String>) -> Self {
        let client: Arc<dyn AtlassianApi> = Arc::new(crate::client::AtlassianClient::new());
        Self::with_client(client, sdk_client, webhook_url)
    }

    pub fn with_client(
        client: Arc<dyn AtlassianApi>,
        sdk_client: SdkClient,
        webhook_url: Option<String>,
    ) -> Self {
        Self {
            sdk_client,
            auth_manager: AuthManager::new(),
            client,
            webhook_url,
        }
    }

    /// Execute a sync driven by the SDK. Delegates lifecycle (complete / fail
    /// / cancel) to the SDK's `SyncContext`: return `Ok(())` for success and
    /// `Err` for failure — the SDK auto-fails on `Err` and the cancel path
    /// below reports `cancelled` explicitly.
    pub async fn run_sync(
        &self,
        _source: Source,
        _credentials: Option<ServiceCredential>,
        state: Option<AtlassianConnectorState>,
        ctx: SyncContext,
    ) -> Result<()> {
        let sync_run_id = ctx.sync_run_id().to_string();
        let source_id = ctx.source_id().to_string();

        info!(
            "Starting sync for source: {} (sync_run_id: {})",
            source_id, sync_run_id
        );

        let outcome = self
            .run_sync_inner(&source_id, &sync_run_id, &ctx, state)
            .await;

        match outcome {
            Ok(Some(_total_processed)) => {
                ctx.complete().await?;
                Ok(())
            }
            // Cancelled mid-flight: report `cancelled` rather than `failed`.
            Ok(None) => {
                info!("Sync {} was cancelled", sync_run_id);
                ctx.cancel().await?;
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    /// Inner sync body. Returns `Ok(None)` if the sync was cancelled
    /// mid-flight, distinct from a successful completion or a hard failure.
    async fn run_sync_inner(
        &self,
        source_id: &str,
        sync_run_id: &str,
        ctx: &SyncContext,
        state: Option<AtlassianConnectorState>,
    ) -> Result<Option<u32>> {
        let source = self
            .sdk_client
            .get_source(source_id)
            .await
            .context("Failed to fetch source via SDK")?;

        if !source.is_active {
            return Err(anyhow!("Source is not active: {}", source_id));
        }

        let source_type = source.source_type;
        if source_type != SourceType::Confluence && source_type != SourceType::Jira {
            return Err(anyhow!(
                "Invalid source type for Atlassian connector: {:?}",
                source_type
            ));
        }

        let project_filters: Option<Vec<String>> = if source_type == SourceType::Jira {
            serde_json::from_value::<JiraSourceConfig>(source.config.clone())
                .ok()
                .and_then(|c| c.project_filters)
                .filter(|f| !f.is_empty())
        } else {
            None
        };

        let space_filters: Option<Vec<String>> = if source_type == SourceType::Confluence {
            serde_json::from_value::<ConfluenceSourceConfig>(source.config.clone())
                .ok()
                .and_then(|c| c.space_filters)
                .filter(|f| !f.is_empty())
        } else {
            None
        };

        let service_creds = self.get_service_credentials(source_id).await?;
        let (domain, sa_token, org_id, org_admin_api_key) =
            self.extract_atlassian_credentials(&service_creds)?;

        debug!("Validating Atlassian credentials...");
        let mut credentials = self
            .get_or_validate_credentials(&domain, &sa_token, Some(&source_type))
            .await?;
        if let (Some(org), Some(key)) = (org_id, org_admin_api_key) {
            credentials = credentials.with_org_admin(org, key);
        }
        self.auth_manager
            .ensure_valid_credentials(&mut credentials, Some(&source_type))
            .await?;
        debug!(
            "Successfully validated Atlassian credentials (org_admin configured: {}).",
            credentials.has_org_admin()
        );

        // Pre-fetch the org-admin user + group directories once per sync. Both
        // are empty when org-admin creds are not configured (resolver falls
        // back to per-site bulk-user API).
        let user_directory: Arc<HashMap<String, String>> = Arc::new(
            self.client
                .get_org_user_directory(&credentials)
                .await
                .unwrap_or_else(|e| {
                    warn!("Failed to fetch org user directory: {}", e);
                    HashMap::new()
                }),
        );
        let group_directory: HashMap<String, OrgGroupInfo> = self
            .client
            .get_org_group_directory(&credentials)
            .await
            .unwrap_or_else(|e| {
                warn!("Failed to fetch org group directory: {}", e);
                HashMap::new()
            });
        if credentials.has_org_admin() {
            info!(
                "Loaded org directory: {} users, {} groups",
                user_directory.len(),
                group_directory.len()
            );
        }
        let user_resolver = Arc::new(UserResolver::new(self.client.clone(), user_directory));

        let existing_state = state.unwrap_or_default();
        let sync_mode = ctx.sync_mode();
        let sync_start = Utc::now();
        let last_sync = existing_state
            .last_successful_sync_at
            .unwrap_or_else(|| sync_start - chrono::Duration::hours(24));

        // The SDK server constructs its own SdkClient for the SyncContext, so
        // emit/flush must go through `ctx.sdk_client()` — using a different
        // SdkClient instance would buffer events on a client whose buffer
        // ctx.complete()/ctx.save_connector_state() never flush, stranding
        // events at end-of-sync.
        let sync_sdk_client = ctx.sdk_client().clone();

        let (total_processed, new_page_versions, encountered_groups) = match source_type {
            SourceType::Confluence => {
                let processor = ConfluenceProcessor::with_page_versions_and_resolver(
                    self.client.clone(),
                    sync_sdk_client.clone(),
                    existing_state.confluence_page_versions.clone(),
                    user_resolver.clone(),
                );
                let result = if sync_mode == SyncType::Full {
                    info!(
                        "Performing full Confluence sync for source: {}",
                        source.name
                    );
                    processor
                        .sync_all_spaces(&credentials, source_id, sync_run_id, ctx, &space_filters)
                        .await
                } else {
                    info!(
                        "Performing incremental Confluence sync for source: {}",
                        source.name
                    );
                    processor
                        .sync_all_spaces_incremental(
                            &credentials,
                            source_id,
                            sync_run_id,
                            last_sync,
                            ctx,
                            &space_filters,
                        )
                        .await
                };
                let count = result?;
                let groups = processor.drain_encountered_groups();
                (count, processor.drain_page_versions(), groups)
            }
            SourceType::Jira => {
                let processor = JiraProcessor::with_resolver(
                    self.client.clone(),
                    sync_sdk_client.clone(),
                    user_resolver.clone(),
                );
                let result = if sync_mode == SyncType::Full {
                    info!("Performing full Jira sync for source: {}", source.name);
                    processor
                        .sync_all_projects(
                            &credentials,
                            source_id,
                            sync_run_id,
                            ctx,
                            &project_filters,
                        )
                        .await
                } else {
                    info!(
                        "Performing incremental Jira sync for source: {}",
                        source.name
                    );
                    processor
                        .sync_issues_updated_since(
                            &credentials,
                            source_id,
                            last_sync,
                            project_filters.as_ref(),
                            sync_run_id,
                            ctx,
                        )
                        .await
                };
                let count = result?;
                let groups = processor.drain_encountered_groups();
                (
                    count,
                    existing_state.confluence_page_versions.clone(),
                    groups,
                )
            }
            _ => unreachable!(),
        };

        if ctx.is_cancelled() {
            return Ok(None);
        }

        self.sync_group_memberships(
            &credentials,
            source_id,
            sync_run_id,
            source_type,
            encountered_groups,
            &group_directory,
            &user_resolver,
            &sync_sdk_client,
        )
        .await;

        info!(
            "Sync completed for source {}: {} documents processed",
            source.name, total_processed
        );

        // ensure_webhook_registered may write webhook_id to connector state; we
        // re-read state afterward so our checkpoint preserves any change.
        if let Err(e) = self
            .ensure_webhook_registered(source_id, &credentials)
            .await
        {
            warn!("Failed to register webhook for source {}: {}", source_id, e);
        }

        let post_webhook_state: AtlassianConnectorState = self
            .sdk_client
            .get_connector_state(source_id)
            .await
            .ok()
            .flatten()
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default();

        let new_state = AtlassianConnectorState {
            webhook_id: post_webhook_state.webhook_id.or(existing_state.webhook_id),
            last_successful_sync_at: Some(sync_start),
            confluence_page_versions: new_page_versions,
        };
        ctx.save_connector_state(serde_json::to_value(new_state)?)
            .await?;

        Ok(Some(total_processed))
    }

    /// For each groupId encountered during the sync, fetch its members,
    /// resolve them to emails, and emit one GroupMembershipSync event. The
    /// authz layer at query time joins these against doc permissions that
    /// reference groupIds in `permissions->'groups'`.
    ///
    /// Per-group failures are warned and skipped so a single bad group can't
    /// abort the membership sync. We do not fail the overall sync run on
    /// errors here — documents have already been emitted.
    pub async fn sync_group_memberships(
        &self,
        creds: &AtlassianCredentials,
        source_id: &str,
        sync_run_id: &str,
        source_type: SourceType,
        encountered_groups: HashMap<String, Option<String>>,
        group_directory: &HashMap<String, OrgGroupInfo>,
        user_resolver: &UserResolver,
        sdk_client: &SdkClient,
    ) {
        if encountered_groups.is_empty() {
            return;
        }

        info!(
            "Emitting group membership events for {} groups (source: {})",
            encountered_groups.len(),
            source_id
        );

        for (group_id, encountered_name) in encountered_groups {
            // Prefer the org-admin group directory when available — it
            // exposes members for every directory user, including those
            // with private email visibility. Falls back to the per-site
            // group-member API when the org directory is empty (org-admin
            // not configured) or the specific group isn't in it.
            let (member_account_ids, group_name) = match group_directory.get(&group_id) {
                Some(info) => (
                    info.member_account_ids.clone(),
                    info.name.clone().or(encountered_name),
                ),
                None => {
                    let fetched = match source_type {
                        SourceType::Confluence => {
                            self.client
                                .get_confluence_group_members(creds, &group_id)
                                .await
                        }
                        SourceType::Jira => {
                            self.client.get_jira_group_members(creds, &group_id).await
                        }
                        _ => unreachable!(),
                    };
                    match fetched {
                        Ok(ids) => (ids, encountered_name),
                        Err(e) => {
                            warn!(
                                "Failed to fetch members for group {} (source: {}): {}",
                                group_id, source_id, e
                            );
                            continue;
                        }
                    }
                }
            };

            let mut member_emails = Vec::new();
            if !member_account_ids.is_empty() {
                match user_resolver
                    .resolve_emails(creds, &member_account_ids)
                    .await
                {
                    Ok(id_email_pairs) => {
                        member_emails.extend(id_email_pairs.into_iter().map(|(_, email)| email));
                    }
                    Err(e) => {
                        warn!(
                            "Failed to resolve member emails for group {}: {}",
                            group_id, e
                        );
                        continue;
                    }
                }
            }
            member_emails.sort();
            member_emails.dedup();

            // FIXME(groups-key-rename): we're storing an Atlassian groupId in the
            // `groups.email` column. Rename that column (and the GroupMembershipSync
            // `group_email` field) to a generic identifier so non-email-keyed group
            // systems are supported first-class.
            let event = ConnectorEvent::GroupMembershipSync {
                sync_run_id: sync_run_id.to_string(),
                source_id: source_id.to_string(),
                group_email: group_id.clone(),
                group_name,
                member_emails,
            };

            if let Err(e) = sdk_client.emit_event(sync_run_id, source_id, event).await {
                warn!(
                    "Failed to emit GroupMembershipSync event for group {}: {}",
                    group_id, e
                );
            }
        }
    }

    async fn get_service_credentials(&self, source_id: &str) -> Result<ServiceCredential> {
        let creds = self
            .sdk_client
            .get_credentials(source_id)
            .await
            .context("Failed to fetch credentials via SDK")?;

        if creds.provider != ServiceProvider::Atlassian {
            return Err(anyhow::anyhow!(
                "Expected Atlassian credentials for source {}, found {:?}",
                source_id,
                creds.provider
            ));
        }

        Ok(creds)
    }

    fn extract_atlassian_credentials(
        &self,
        creds: &ServiceCredential,
    ) -> Result<(String, String, Option<String>, Option<String>)> {
        let domain = creds
            .config
            .get("domain")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing domain in service credentials config"))?
            .to_string();

        let sa_token = creds
            .credentials
            .get("sa_token")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing sa_token in service credentials"))?
            .to_string();

        // Optional: organization-admin credentials enable the org-admin
        // identity-resolution path. When absent the connector falls back to
        // the per-site bulk-user API for accountId → email resolution.
        let org_id = creds
            .config
            .get("org_id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let org_admin_api_key = creds
            .credentials
            .get("org_admin_api_key")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        Ok((domain, sa_token, org_id, org_admin_api_key))
    }

    async fn get_or_validate_credentials(
        &self,
        domain: &str,
        sa_token: &str,
        source_type: Option<&SourceType>,
    ) -> Result<AtlassianCredentials> {
        self.auth_manager
            .validate_credentials(domain, sa_token, source_type)
            .await
    }

    pub async fn ensure_webhook_registered(
        &self,
        source_id: &str,
        creds: &AtlassianCredentials,
    ) -> Result<()> {
        let webhook_url = match &self.webhook_url {
            Some(url) => url,
            None => return Ok(()),
        };

        let state: AtlassianConnectorState = self
            .sdk_client
            .get_connector_state(source_id)
            .await
            .ok()
            .flatten()
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default();

        if let Some(webhook_id) = state.webhook_id {
            match self.client.get_webhook(creds, webhook_id).await {
                Ok(true) => {
                    debug!(
                        "Webhook {} still exists for source {}",
                        webhook_id, source_id
                    );
                    return Ok(());
                }
                Ok(false) => {
                    info!("Webhook {} no longer exists, re-registering", webhook_id);
                }
                Err(e) => {
                    warn!(
                        "Failed to check webhook {}: {}, re-registering",
                        webhook_id, e
                    );
                }
            }
        }

        let full_url = format!("{}?source_id={}", webhook_url, source_id);
        let webhook_id = self.client.register_webhook(creds, &full_url).await?;
        info!("Registered webhook {} for source {}", webhook_id, source_id);

        // Preserve other state fields — run_sync writes page versions and
        // last_successful_sync_at, which we must not clobber from this path.
        let new_state = AtlassianConnectorState {
            webhook_id: Some(webhook_id),
            ..state
        };
        self.sdk_client
            .save_connector_state(source_id, serde_json::to_value(&new_state)?)
            .await?;

        Ok(())
    }

    pub async fn handle_webhook_event(
        &self,
        source_id: &str,
        event: AtlassianWebhookEvent,
    ) -> Result<()> {
        info!(
            "Handling webhook event '{}' for source {}",
            event.webhook_event, source_id
        );

        match event.webhook_event.as_str() {
            "jira:issue_deleted" => {
                let Some(issue) = &event.issue else {
                    return Ok(());
                };
                let project_key = issue
                    .fields
                    .as_ref()
                    .and_then(|f| f.project.as_ref())
                    .map(|p| p.key.as_str())
                    .unwrap_or("");

                if project_key.is_empty() {
                    warn!("Cannot delete issue without project key");
                    return Ok(());
                }

                self.emit_delete(
                    source_id,
                    format!("jira_issue_{}_{}", project_key, issue.key),
                )
                .await
            }
            "page_removed" | "page_trashed" => {
                let Some(page) = &event.page else {
                    return Ok(());
                };
                let space_key = page
                    .space_key
                    .as_deref()
                    .or_else(|| page.space.as_ref().map(|s| s.key.as_str()))
                    .unwrap_or("");

                if space_key.is_empty() {
                    warn!("Cannot delete page without space key");
                    return Ok(());
                }

                self.emit_delete(
                    source_id,
                    format!("confluence_page_{}_{}", space_key, page.id),
                )
                .await
            }
            "jira:issue_created" | "jira:issue_updated" | "page_created" | "page_updated" => {
                self.sdk_client
                    .notify_webhook(source_id, &event.webhook_event)
                    .await?;
                Ok(())
            }
            _ => {
                debug!("Ignoring unhandled webhook event: {}", event.webhook_event);
                Ok(())
            }
        }
    }

    /// Create a one-off sync run, emit a single DocumentDeleted event, and
    /// close the run. Used by webhook-driven deletes.
    async fn emit_delete(&self, source_id: &str, document_id: String) -> Result<()> {
        let sync_run_id = self
            .sdk_client
            .create_sync_run(source_id, SyncType::Incremental)
            .await?;

        let event = ConnectorEvent::DocumentDeleted {
            sync_run_id: sync_run_id.clone(),
            source_id: source_id.to_string(),
            document_id,
        };

        let result = self
            .sdk_client
            .emit_event(&sync_run_id, source_id, event)
            .await
            .map_err(Into::into);

        match &result {
            Ok(_) => {
                self.sdk_client.increment_scanned(&sync_run_id, 1).await?;
                self.sdk_client.increment_updated(&sync_run_id, 1).await?;
                self.sdk_client.complete(&sync_run_id).await?;
            }
            Err(e) => {
                self.sdk_client
                    .fail(&sync_run_id, &format!("{}", e))
                    .await?;
            }
        }
        result
    }

    pub async fn ensure_webhooks_for_all_sources(&self) {
        let source_types = ["confluence", "jira"];

        for source_type in &source_types {
            let sources = match self.sdk_client.get_sources_by_type(source_type).await {
                Ok(s) => s,
                Err(e) => {
                    debug!("Failed to list {:?} sources: {}", source_type, e);
                    continue;
                }
            };

            for source in sources {
                let source_id = &source.id;
                let service_creds = match self.get_service_credentials(source_id).await {
                    Ok(c) => c,
                    Err(e) => {
                        debug!("Failed to get credentials for source {}: {}", source_id, e);
                        continue;
                    }
                };

                let (domain, sa_token, _org_id, _org_admin_api_key) =
                    match self.extract_atlassian_credentials(&service_creds) {
                        Ok(c) => c,
                        Err(e) => {
                            debug!("Failed to extract credentials for {}: {}", source_id, e);
                            continue;
                        }
                    };

                let creds = match self
                    .get_or_validate_credentials(&domain, &sa_token, None)
                    .await
                {
                    Ok(c) => c,
                    Err(e) => {
                        debug!("Failed to validate credentials for {}: {}", source_id, e);
                        continue;
                    }
                };

                if let Err(e) = self.ensure_webhook_registered(source_id, &creds).await {
                    warn!("Failed to ensure webhook for source {}: {}", source_id, e);
                }
            }
        }
    }
}
