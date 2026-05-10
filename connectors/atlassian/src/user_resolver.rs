use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;

use crate::auth::AtlassianCredentials;
use crate::client::AtlassianApi;

/// Resolves Atlassian `accountId` values to email addresses, preferring a
/// pre-fetched org-admin directory (which surfaces emails for users with
/// privacy settings enabled) and falling back to the per-site bulk-user API
/// for accountIds not in the directory (apps, external collaborators, etc.).
///
/// One instance per sync; the `directory` map is read-only and shared across
/// processors via `Arc`.
pub struct UserResolver {
    client: Arc<dyn AtlassianApi>,
    directory: Arc<HashMap<String, String>>,
}

impl UserResolver {
    pub fn new(client: Arc<dyn AtlassianApi>, directory: Arc<HashMap<String, String>>) -> Self {
        Self { client, directory }
    }

    /// Returns `(accountId, email)` for as many of the requested accountIds
    /// as we can resolve. AccountIds with no resolvable email (e.g., apps, or
    /// users whose email is hidden AND not in the org directory) are silently
    /// dropped — they cannot be matched against Omni's email-keyed authz.
    pub async fn resolve_emails(
        &self,
        creds: &AtlassianCredentials,
        account_ids: &[String],
    ) -> Result<Vec<(String, String)>> {
        let mut resolved: Vec<(String, String)> = Vec::with_capacity(account_ids.len());
        let mut unresolved: Vec<String> = Vec::new();

        for account_id in account_ids {
            match self.directory.get(account_id) {
                Some(email) => resolved.push((account_id.clone(), email.clone())),
                None => unresolved.push(account_id.clone()),
            }
        }

        if !unresolved.is_empty() {
            let bulk = self.client.get_jira_users_bulk(creds, &unresolved).await?;
            resolved.extend(bulk);
        }

        Ok(resolved)
    }
}
