use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use dashmap::DashMap;
use omni_connector_sdk::{
    ConnectorEvent, SdkClient, ServiceCredential, Source, SourceType, SyncContext, SyncType,
};
use shared::models::{ConfluenceSourceConfig, JiraSourceConfig, ServiceProvider};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tracing::{debug, info, warn};

use crate::auth::{AtlassianCredentials, AuthManager};
use crate::client::AtlassianApi;
use crate::confluence::ConfluenceProcessor;
use crate::jira::JiraProcessor;
use crate::models::{AtlassianConnectorState, AtlassianWebhookEvent};

pub struct SyncManager {
    pub sdk_client: SdkClient,
    auth_manager: AuthManager,
    client: Arc<dyn AtlassianApi>,
    active_syncs: DashMap<String, Arc<AtomicBool>>,
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
            active_syncs: DashMap::new(),
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
        _credentials: Option<ServiceCredentials>,
        state: Option<AtlassianConnectorState>,
        ctx: SyncContext,
    ) -> Result<()> {
        let sync_run_id = ctx.sync_run_id().to_string();
        let source_id = ctx.source_id().to_string();

        info!(
            "Starting sync for source: {} (sync_run_id: {})",
            source_id, sync_run_id
        );

        // Mirror the SDK's cancellation flag so nested helpers can poll it
        // through the existing `&AtomicBool` signatures without threading
        // `SyncContext` everywhere.
        let cancelled = Arc::new(AtomicBool::new(false));
        let cancel_bridge = {
            let cancelled = cancelled.clone();
            let ctx = ctx.clone();
            tokio::spawn(async move {
                while !ctx.is_cancelled() {
                    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                }
                cancelled.store(true, Ordering::SeqCst);
            })
        };
        self.active_syncs
            .insert(sync_run_id.clone(), cancelled.clone());

        let outcome = self
            .run_sync_inner(&source_id, &sync_run_id, &cancelled, &ctx, state)
            .await;
        self.active_syncs.remove(&sync_run_id);
        cancel_bridge.abort();

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
        cancelled: &AtomicBool,
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
        let (base_url, user_email, api_token) =
            self.extract_atlassian_credentials(&service_creds)?;

        debug!("Validating Atlassian credentials...");
        let mut credentials = self
            .get_or_validate_credentials(&base_url, &user_email, &api_token, Some(&source_type))
            .await?;
        self.auth_manager
            .ensure_valid_credentials(&mut credentials, Some(&source_type))
            .await?;
        debug!("Successfully validated Atlassian credentials.");

        let existing_state = state.unwrap_or_default();
        let sync_mode = ctx.sync_mode();
        let sync_start = Utc::now();
        let last_sync = existing_state
            .last_successful_sync_at
            .unwrap_or_else(|| sync_start - chrono::Duration::hours(24));

        let (total_processed, new_page_versions) = match source_type {
            SourceType::Confluence => {
                let processor = ConfluenceProcessor::with_page_versions(
                    self.client.clone(),
                    self.sdk_client.clone(),
                    existing_state.confluence_page_versions.clone(),
                );
                let result = if sync_mode == SyncType::Full {
                    info!(
                        "Performing full Confluence sync for source: {}",
                        source.name
                    );
                    processor
                        .sync_all_spaces(
                            &credentials,
                            source_id,
                            sync_run_id,
                            cancelled,
                            &space_filters,
                        )
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
                            cancelled,
                            &space_filters,
                        )
                        .await
                };
                let count = result?;
                (count, processor.drain_page_versions())
            }
            SourceType::Jira => {
                let processor = JiraProcessor::new(self.client.clone(), self.sdk_client.clone());
                let result = if sync_mode == SyncType::Full {
                    info!("Performing full Jira sync for source: {}", source.name);
                    processor
                        .sync_all_projects(
                            &credentials,
                            source_id,
                            sync_run_id,
                            cancelled,
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
                            cancelled,
                        )
                        .await
                };
                let count = result?;
                (count, existing_state.confluence_page_versions.clone())
            }
            _ => unreachable!(),
        };

        if cancelled.load(Ordering::SeqCst) {
            return Ok(None);
        }

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
    ) -> Result<(String, String, String)> {
        let base_url = creds
            .config
            .get("base_url")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing base_url in service credentials config"))?
            .to_string();

        let user_email = creds
            .principal_email
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Missing principal_email in service credentials"))?
            .to_string();

        let api_token = creds
            .credentials
            .get("api_token")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing api_token in service credentials"))?
            .to_string();

        Ok((base_url, user_email, api_token))
    }

    async fn get_or_validate_credentials(
        &self,
        base_url: &str,
        user_email: &str,
        api_token: &str,
        source_type: Option<&SourceType>,
    ) -> Result<AtlassianCredentials> {
        self.auth_manager
            .validate_credentials(base_url, user_email, api_token, source_type)
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

                let (base_url, user_email, api_token) =
                    match self.extract_atlassian_credentials(&service_creds) {
                        Ok(c) => c,
                        Err(e) => {
                            debug!("Failed to extract credentials for {}: {}", source_id, e);
                            continue;
                        }
                    };

                let creds = match self
                    .get_or_validate_credentials(&base_url, &user_email, &api_token, None)
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
