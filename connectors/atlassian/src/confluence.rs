use anyhow::Result;
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use futures::stream::StreamExt;
use shared::models::DocumentPermissions;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tracing::{debug, error, info, warn};

use crate::auth::AtlassianCredentials;
use crate::client::AtlassianApi;
use crate::models::{ConfluencePage, ConfluencePageStatus, ConfluenceSpace};
use shared::SdkClient;

pub struct ConfluenceProcessor {
    client: Arc<dyn AtlassianApi>,
    sdk_client: SdkClient,
    space_permissions_cache: DashMap<String, DocumentPermissions>,
    /// Last-indexed version per page, keyed by `{space_id}:{page_id}`. Seeded
    /// from connector state at sync start; the SyncManager drains this into
    /// the new state after a successful run.
    page_versions: DashMap<String, i32>,
}

fn page_version_key(space_id: &str, page_id: &str) -> String {
    format!("{}:{}", space_id, page_id)
}

impl ConfluenceProcessor {
    pub fn new(client: Arc<dyn AtlassianApi>, sdk_client: SdkClient) -> Self {
        Self::with_page_versions(client, sdk_client, HashMap::new())
    }

    pub fn with_page_versions(
        client: Arc<dyn AtlassianApi>,
        sdk_client: SdkClient,
        page_versions: HashMap<String, i32>,
    ) -> Self {
        Self {
            client,
            sdk_client,
            space_permissions_cache: DashMap::new(),
            page_versions: page_versions.into_iter().collect(),
        }
    }

    /// Drain the current version map into a plain HashMap so the SyncManager
    /// can persist it on the connector state after a successful sync.
    pub fn drain_page_versions(&self) -> HashMap<String, i32> {
        self.page_versions
            .iter()
            .map(|entry| (entry.key().clone(), *entry.value()))
            .collect()
    }

    async fn get_space_permissions(
        &self,
        creds: &AtlassianCredentials,
        space_id: &str,
    ) -> DocumentPermissions {
        if let Some(cached) = self.space_permissions_cache.get(space_id) {
            return cached.clone();
        }

        let perms = match self.fetch_space_permissions(creds, space_id).await {
            Ok(p) => p,
            Err(e) => {
                warn!(
                    "Failed to fetch permissions for space {}, defaulting to public: {}",
                    space_id, e
                );
                DocumentPermissions {
                    public: true,
                    users: vec![],
                    groups: vec![],
                }
            }
        };

        self.space_permissions_cache
            .insert(space_id.to_string(), perms.clone());
        perms
    }

    async fn fetch_space_permissions(
        &self,
        creds: &AtlassianCredentials,
        space_id: &str,
    ) -> Result<DocumentPermissions> {
        // TODO(perms): No domain-restriction check — if the Atlassian org limits
        // a space to a specific email domain we don't enforce that here.
        let permissions = self
            .client
            .get_confluence_space_permissions(creds, space_id)
            .await?;

        // Filter for read permissions on the space
        let read_perms: Vec<_> = permissions
            .iter()
            .filter(|p| p.operation.key == "read" && p.operation.target == "space")
            .collect();

        // If no explicit read permissions are returned, the space is likely open to all
        // org members (Confluence Cloud default). Safer to over-expose than silently hide.
        //
        // TODO(perms): This conflates "no perms" with "public". We should call
        // the space-settings endpoint to distinguish "open to all org members"
        // from "anonymous access enabled" from "inheritance-only".
        if read_perms.is_empty() {
            debug!(
                "No read permissions found for space {}, marking as public",
                space_id
            );
            return Ok(DocumentPermissions {
                public: true,
                users: vec![],
                groups: vec![],
            });
        }

        let mut user_account_ids = Vec::new();
        let mut group_ids = Vec::new();

        for perm in &read_perms {
            match perm.principal.principal_type.as_str() {
                "user" => {
                    user_account_ids.push(perm.principal.id.clone());
                }
                "group" => {
                    group_ids.push(perm.principal.id.clone());
                }
                _ => {}
            }
        }

        // Resolve user accountIds to emails via bulk API
        let mut user_emails = Vec::new();
        if !user_account_ids.is_empty() {
            match self
                .client
                .get_jira_users_bulk(creds, &user_account_ids)
                .await
            {
                Ok(id_email_pairs) => {
                    user_emails.extend(id_email_pairs.into_iter().map(|(_, email)| email));
                }
                Err(e) => {
                    warn!(
                        "Failed to resolve user emails for space {}: {}",
                        space_id, e
                    );
                }
            }
        }

        // Resolve group IDs to member emails
        for group_id in &group_ids {
            match self
                .client
                .get_confluence_group_members(creds, group_id)
                .await
            {
                Ok(member_emails) => {
                    user_emails.extend(member_emails);
                }
                Err(e) => {
                    warn!(
                        "Failed to fetch members for group {} in space {}: {}",
                        group_id, space_id, e
                    );
                }
            }
        }

        user_emails.sort();
        user_emails.dedup();

        // TODO(perms): Groups are expanded to member emails at sync time, so we
        // emit groups: vec![] for the authz service. This scales poorly for
        // large groups (N API calls + unbounded member lists). Long-term, emit
        // group identifiers and resolve membership in the authz layer.
        Ok(DocumentPermissions {
            public: false,
            users: user_emails,
            groups: vec![],
        })
    }

    pub async fn sync_all_spaces_incremental(
        &self,
        creds: &AtlassianCredentials,
        source_id: &str,
        sync_run_id: &str,
        last_sync: DateTime<Utc>,
        cancelled: &AtomicBool,
        space_filters: &Option<Vec<String>>,
    ) -> Result<u32> {
        info!(
            "Starting incremental Confluence sync for source: {} since {}{} (sync_run_id: {})",
            source_id,
            last_sync.format("%Y-%m-%d %H:%M"),
            space_filters
                .as_ref()
                .map_or(String::new(), |f| format!(" (spaces: {:?})", f)),
            sync_run_id
        );

        let mut cql = format!(
            "lastModified >= \"{}\" AND type = page",
            last_sync.format("%Y-%m-%d %H:%M")
        );
        if let Some(filters) = space_filters {
            if !filters.is_empty() {
                let spaces_str = filters
                    .iter()
                    .map(|s| format!("\"{}\"", s))
                    .collect::<Vec<_>>()
                    .join(", ");
                cql = format!("space IN ({}) AND {}", spaces_str, cql);
            }
        }

        let mut total_pages_processed = 0;

        // Collect all pages first to avoid borrow conflicts with process_pages
        let mut all_pages = Vec::new();
        {
            let mut stream = self.client.search_confluence_pages_by_cql(creds, &cql);
            while let Some(result) = stream.next().await {
                if cancelled.load(Ordering::SeqCst) {
                    info!(
                        "Confluence incremental sync {} cancelled after {} pages",
                        sync_run_id, total_pages_processed
                    );
                    return Ok(total_pages_processed);
                }

                let cql_page = result?;
                if let Some(page) = cql_page.into_confluence_page() {
                    all_pages.push(page);
                }
            }
        }

        for batch in all_pages.chunks(100) {
            let count = self
                .process_pages(
                    batch.to_vec(),
                    source_id,
                    sync_run_id,
                    &creds.base_url,
                    creds,
                )
                .await?;
            total_pages_processed += count;
            if let Err(e) = self
                .sdk_client
                .increment_scanned(sync_run_id, count as i32)
                .await
            {
                error!("Failed to increment scanned count: {}", e);
            }
        }

        info!(
            "Completed incremental Confluence sync. Pages processed: {}",
            total_pages_processed
        );
        Ok(total_pages_processed)
    }

    pub async fn sync_all_spaces(
        &self,
        creds: &AtlassianCredentials,
        source_id: &str,
        sync_run_id: &str,
        cancelled: &AtomicBool,
        space_filters: &Option<Vec<String>>,
    ) -> Result<u32> {
        info!(
            "Starting full Confluence sync for source: {} (sync_run_id: {})",
            source_id, sync_run_id
        );

        let all_spaces = self.get_accessible_spaces(creds).await?;
        let spaces: Vec<ConfluenceSpace> = match space_filters {
            Some(filters) => {
                let filtered: Vec<ConfluenceSpace> = all_spaces
                    .into_iter()
                    .filter(|s| filters.iter().any(|f| f.eq_ignore_ascii_case(&s.key)))
                    .collect();
                info!(
                    "Filtered to {} spaces (from {} accessible)",
                    filtered.len(),
                    filters.len()
                );
                filtered
            }
            None => all_spaces,
        };
        let mut total_pages_processed = 0;

        for space in spaces {
            if cancelled.load(Ordering::SeqCst) {
                info!(
                    "Confluence sync {} cancelled, stopping early after {} pages",
                    sync_run_id, total_pages_processed
                );
                return Ok(total_pages_processed);
            }

            info!(
                "Syncing Confluence space: {} [key={}, id={}]",
                space.name, space.key, space.id
            );

            match self
                .sync_space_pages(creds, source_id, sync_run_id, &space.id, cancelled)
                .await
            {
                Ok(pages_count) => {
                    total_pages_processed += pages_count;
                    info!("Synced {} pages from space: {}", pages_count, space.id);
                    if let Err(e) = self
                        .sdk_client
                        .increment_scanned(sync_run_id, pages_count as i32)
                        .await
                    {
                        error!("Failed to increment scanned count: {}", e);
                    }
                }
                Err(e) => {
                    error!("Failed to sync space {}: {}", space.id, e);
                }
            }
        }

        info!(
            "Completed Confluence sync. Total pages processed: {}",
            total_pages_processed
        );
        Ok(total_pages_processed)
    }

    async fn sync_space_pages(
        &self,
        creds: &AtlassianCredentials,
        source_id: &str,
        sync_run_id: &str,
        space_id: &str,
        cancelled: &AtomicBool,
    ) -> Result<u32> {
        let mut total_pages = 0;

        info!("Fetching pages for Confluence space {}", space_id);

        // Collect all pages first to avoid borrow conflicts with process_pages
        let mut all_pages = Vec::new();
        {
            let mut pages_stream = self.client.get_confluence_pages(creds, space_id);
            while let Some(page_result) = pages_stream.next().await {
                if cancelled.load(Ordering::SeqCst) {
                    info!(
                        "Confluence sync cancelled during space {} page streaming",
                        space_id
                    );
                    return Ok(total_pages);
                }
                all_pages.push(page_result?);
            }
        }

        for batch in all_pages.chunks(100) {
            let count = self
                .process_pages(
                    batch.to_vec(),
                    source_id,
                    sync_run_id,
                    &creds.base_url,
                    creds,
                )
                .await?;
            total_pages += count;
        }

        info!(
            "Processed {} pages from Confluence space {}",
            total_pages, space_id
        );
        Ok(total_pages)
    }

    async fn get_accessible_spaces(
        &self,
        creds: &AtlassianCredentials,
    ) -> Result<Vec<ConfluenceSpace>> {
        let spaces = self.client.get_confluence_spaces(creds).await?;
        if spaces.is_empty() {
            debug!("No spaces found for Confluence instance {}", creds.base_url);
        }
        debug!("Found {} accessible Confluence spaces", spaces.len());
        Ok(spaces)
    }

    async fn process_pages(
        &self,
        pages: Vec<ConfluencePage>,
        source_id: &str,
        sync_run_id: &str,
        base_url: &str,
        creds: &AtlassianCredentials,
    ) -> Result<u32> {
        let mut count = 0;

        for page in pages {
            // Skip non-current pages (drafts, trashed, etc.)
            if page.status != ConfluencePageStatus::Current {
                debug!("Skipping page {} with status: {:?}", page.id, page.status);
                continue;
            }

            // Skip pages whose version hasn't changed since the last
            // successful sync (state seeded in-memory at sync start).
            let current_version = page.version.number;
            let version_key = page_version_key(&page.space_id, &page.id);
            let should_process = match self.page_versions.get(&version_key) {
                Some(entry) => {
                    let last_version = *entry;
                    if last_version != current_version {
                        debug!(
                            "Page {} has been updated (was version {}, now version {})",
                            page.title, last_version, current_version
                        );
                        true
                    } else {
                        debug!(
                            "Skipping page {} - version {} unchanged",
                            page.title, current_version
                        );
                        false
                    }
                }
                None => {
                    debug!("Page {} is new, will process", page.title);
                    true
                }
            };

            if !should_process {
                continue;
            }

            // Skip pages without content
            let content = page.extract_plain_text();
            if content.trim().is_empty() {
                debug!("Skipping page {} without content", page.id);
                continue;
            }

            debug!(
                "Processing Confluence page: {} in space {} (content length: {} chars)",
                page.title,
                page.space_id,
                content.len()
            );

            // Store content via SDK
            let content_id = match self.sdk_client.store_content(sync_run_id, &content).await {
                Ok(id) => id,
                Err(e) => {
                    error!(
                        "Failed to store content via SDK for Confluence page {}: {}",
                        page.title, e
                    );
                    continue;
                }
            };

            let permissions = self.get_space_permissions(creds, &page.space_id).await;

            let event = page.to_connector_event(
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
                error!(
                    "Failed to emit event for Confluence page {}: {}",
                    page.title, e
                );
                continue;
            }

            count += 1;

            self.page_versions.insert(version_key, current_version);
        }

        Ok(count)
    }
}
