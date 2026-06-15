use anyhow::{Result, anyhow};
use chrono::Utc;
use omni_connector_sdk::SourceType;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

/// Atlassian service-account credentials. We use Bearer auth against the
/// Atlassian API gateway exclusively — direct site URLs with Basic auth (the
/// legacy user-API-token model) are not supported.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AtlassianCredentials {
    /// Site domain, e.g. "company.atlassian.net". Used to fetch the cloud_id
    /// and to register webhooks (the webhook v1 API is site-scoped, not
    /// gateway-scoped).
    pub domain: String,
    /// Cloud ID for this site, fetched once at validation from
    /// `https://{domain}/_edge/tenant_info`.
    pub cloud_id: String,
    /// Service-account API token. Issued at admin.atlassian.com →
    /// Service accounts. Tokens conventionally start with `ATSTT`.
    pub sa_token: String,
    pub validated_at: i64,
    /// Account ID of the service account itself (populated during validation).
    /// Used to filter the SA out of page/issue restriction lists — the SA is
    /// always present because Atlassian requires the API caller to have access,
    /// but it is not a meaningful permission grant for end-user authz.
    #[serde(default)]
    pub sa_account_id: Option<String>,
    /// Atlassian Organization ID (UUID) — required for the org-admin
    /// directory path. Set when the source has Atlassian Guard provisioned.
    #[serde(default)]
    pub org_id: Option<String>,
    /// Bearer token issued at admin.atlassian.com for the organization.
    /// Authenticates against `api.atlassian.com/admin/v1/...` (different
    /// credential class from the SA token above).
    #[serde(default)]
    pub org_admin_api_key: Option<String>,
}

impl AtlassianCredentials {
    pub fn new(domain: String, cloud_id: String, sa_token: String) -> Self {
        Self {
            domain,
            cloud_id,
            sa_token,
            validated_at: Utc::now().timestamp_millis(),
            sa_account_id: None,
            org_id: None,
            org_admin_api_key: None,
        }
    }

    pub fn with_sa_account_id(mut self, account_id: String) -> Self {
        self.sa_account_id = Some(account_id);
        self
    }

    pub fn with_org_admin(mut self, org_id: String, org_admin_api_key: String) -> Self {
        self.org_id = Some(org_id);
        self.org_admin_api_key = Some(org_admin_api_key);
        self
    }

    pub fn has_org_admin(&self) -> bool {
        self.org_id.is_some() && self.org_admin_api_key.is_some()
    }

    pub fn is_valid(&self) -> bool {
        // SA tokens don't expire (until revoked), but we re-validate every
        // 24 hours to catch revocation / scope changes early.
        let now = Utc::now().timestamp_millis();
        let one_day_ms = 24 * 60 * 60 * 1000;
        (now - self.validated_at) < one_day_ms
    }

    /// Gateway base for all Jira REST calls.
    /// e.g. `https://api.atlassian.com/ex/jira/{cloud_id}`
    pub fn jira_base(&self) -> String {
        format!("https://api.atlassian.com/ex/jira/{}", self.cloud_id)
    }

    /// Gateway base for all Confluence REST calls (includes the `/wiki`
    /// segment, so call sites use `{base}/rest/api/...` and `{base}/api/v2/...`).
    /// e.g. `https://api.atlassian.com/ex/confluence/{cloud_id}/wiki`
    pub fn confluence_base(&self) -> String {
        format!(
            "https://api.atlassian.com/ex/confluence/{}/wiki",
            self.cloud_id
        )
    }

    /// Direct site URL. Used only for cloud_id discovery and webhook
    /// registration (the legacy `/rest/webhooks/1.0/` API is site-scoped).
    pub fn site_base(&self) -> String {
        format!("https://{}", self.domain)
    }

    pub fn get_bearer_auth_header(&self) -> String {
        format!("Bearer {}", self.sa_token)
    }

    /// Bearer header for the org-admin API. Returns None when org-admin
    /// credentials are not configured on this source.
    pub fn get_org_admin_bearer_header(&self) -> Option<String> {
        self.org_admin_api_key
            .as_ref()
            .map(|t| format!("Bearer {}", t))
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AtlassianUserResponse {
    #[serde(rename = "accountId")]
    pub account_id: String,
    #[serde(rename = "displayName")]
    pub display_name: String,
    /// May be null for users with private email visibility — but for service
    /// accounts it is always populated with the auto-generated address
    /// `<sa-name>@serviceaccount.atlassian.com`.
    #[serde(rename = "emailAddress")]
    pub email_address: Option<String>,
    /// Returned for SAs as `"app"`; for humans as `"atlassian"`.
    #[serde(default, rename = "accountType")]
    pub account_type: Option<String>,
    #[serde(default)]
    pub active: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ConfluenceUserInfo {
    #[serde(rename = "accountId")]
    pub account_id: String,
    #[serde(rename = "displayName")]
    pub display_name: String,
}

#[derive(Debug, Deserialize)]
struct TenantInfoResponse {
    #[serde(rename = "cloudId")]
    cloud_id: String,
}

pub struct AuthManager {
    client: Client,
}

impl AuthManager {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
        }
    }

    /// Fetch the cloud_id for an Atlassian site. The `_edge/tenant_info`
    /// endpoint is unauthenticated and exposes only the cloud_id.
    pub async fn fetch_cloud_id(&self, domain: &str) -> Result<String> {
        let url = format!("https://{}/_edge/tenant_info", domain);
        let response = self.client.get(&url).send().await.map_err(|e| {
            anyhow!(
                "Failed to reach Atlassian site {} for cloud_id discovery: {}",
                domain,
                e
            )
        })?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(anyhow!(
                "tenant_info lookup failed for {}: HTTP {} — {}",
                domain,
                status,
                body
            ));
        }

        let info: TenantInfoResponse = response.json().await.map_err(|e| {
            anyhow!(
                "tenant_info response for {} did not parse as JSON: {}",
                domain,
                e
            )
        })?;
        Ok(info.cloud_id)
    }

    pub async fn validate_credentials(
        &self,
        domain: &str,
        sa_token: &str,
        source_type: Option<&SourceType>,
    ) -> Result<AtlassianCredentials> {
        info!("Validating Atlassian credentials for site: {}", domain);

        if !sa_token.starts_with("ATSTT") {
            // Don't fail — Atlassian could change the prefix in the future.
            // Just nudge the operator if they accidentally pasted a user
            // API token (`ATATT…`).
            warn!(
                "SA token does not start with `ATSTT`. \
                 The Atlassian connector requires a service account token. \
                 Continuing — auth will fail at the first API call if this is wrong."
            );
        }

        let cloud_id = self.fetch_cloud_id(domain).await?;
        debug!("Resolved cloud_id {} for site {}", cloud_id, domain);

        let mut creds =
            AtlassianCredentials::new(domain.to_string(), cloud_id, sa_token.to_string());
        let auth_header = creds.get_bearer_auth_header();

        let validate_jira = source_type != Some(&SourceType::Confluence);
        let validate_confluence = source_type != Some(&SourceType::Jira);

        let mut sa_account_id = None;
        if validate_jira {
            let jira_url = format!("{}/rest/api/3/myself", creds.jira_base());
            let jira_response = self
                .client
                .get(&jira_url)
                .header("Authorization", &auth_header)
                .header("Accept", "application/json")
                .send()
                .await?;

            if !jira_response.status().is_success() {
                let status = jira_response.status();
                let error_text = jira_response.text().await?;
                return Err(anyhow!(
                    "Failed to validate Jira credentials via gateway: HTTP {} - {}",
                    status,
                    error_text
                ));
            }

            let jira_user: AtlassianUserResponse = jira_response.json().await?;
            debug!(
                "Jira validation successful for account: {} ({})",
                jira_user.display_name, jira_user.account_id
            );
            if jira_user.active == Some(false) {
                return Err(anyhow!("Service account is not active"));
            }
            info!(
                "Validated Jira access as {} (accountId {}, type {:?})",
                jira_user.display_name, jira_user.account_id, jira_user.account_type
            );
            sa_account_id = Some(jira_user.account_id);
        }

        if validate_confluence {
            let confluence_url = format!("{}/rest/api/user/current", creds.confluence_base());
            let confluence_response = self
                .client
                .get(&confluence_url)
                .header("Authorization", &auth_header)
                .header("Accept", "application/json")
                .send()
                .await?;

            if !confluence_response.status().is_success() {
                let status = confluence_response.status();
                let error_text = confluence_response.text().await?;
                return Err(anyhow!(
                    "Failed to validate Confluence credentials via gateway: HTTP {} - {}",
                    status,
                    error_text
                ));
            }

            let confluence_user: ConfluenceUserInfo = confluence_response.json().await?;
            debug!(
                "Confluence validation successful for account: {} ({})",
                confluence_user.display_name, confluence_user.account_id
            );

            if let Some(ref jira_id) = sa_account_id {
                if confluence_user.account_id != *jira_id {
                    return Err(anyhow!(
                        "Account ID mismatch between Jira and Confluence \
                         (Jira: {}, Confluence: {}) — the SA token must \
                         resolve to the same identity in both products.",
                        jira_id,
                        confluence_user.account_id
                    ));
                }
            }

            // For Confluence-only sources we capture the accountId here.
            if sa_account_id.is_none() {
                sa_account_id = Some(confluence_user.account_id);
            }
        }

        if let Some(account_id) = sa_account_id {
            creds = creds.with_sa_account_id(account_id);
        }

        Ok(creds)
    }

    pub async fn ensure_valid_credentials(
        &self,
        creds: &mut AtlassianCredentials,
        source_type: Option<&SourceType>,
    ) -> Result<()> {
        if !creds.is_valid() {
            debug!("Re-validating SA token");
            let new_creds = self
                .validate_credentials(&creds.domain, &creds.sa_token, source_type)
                .await?;
            // Preserve any org-admin creds and sa_account_id that were
            // attached after validation.
            let sa_account_id = creds.sa_account_id.clone();
            *creds = match (creds.org_id.clone(), creds.org_admin_api_key.clone()) {
                (Some(org), Some(key)) => new_creds.with_org_admin(org, key),
                _ => new_creds,
            };
            if let Some(id) = sa_account_id {
                creds.sa_account_id = Some(id);
            }
        }
        Ok(())
    }

    pub async fn test_jira_permissions(&self, creds: &AtlassianCredentials) -> Result<Vec<String>> {
        let auth_header = creds.get_bearer_auth_header();
        let url = format!("{}/rest/api/3/project", creds.jira_base());

        let response = self
            .client
            .get(&url)
            .header("Authorization", &auth_header)
            .header("Accept", "application/json")
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await?;
            return Err(anyhow!(
                "Failed to fetch Jira projects: HTTP {} - {}",
                status,
                error_text
            ));
        }

        let projects: Vec<serde_json::Value> = response.json().await?;
        let project_keys: Vec<String> = projects
            .iter()
            .filter_map(|p| p.get("key").and_then(|k| k.as_str().map(String::from)))
            .collect();

        debug!("Found {} accessible Jira projects", project_keys.len());
        Ok(project_keys)
    }

    pub async fn test_confluence_permissions(
        &self,
        creds: &AtlassianCredentials,
    ) -> Result<Vec<String>> {
        let auth_header = creds.get_bearer_auth_header();
        let url = format!("{}/rest/api/space?limit=100", creds.confluence_base());

        let response = self
            .client
            .get(&url)
            .header("Authorization", &auth_header)
            .header("Accept", "application/json")
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await?;
            return Err(anyhow!(
                "Failed to fetch Confluence spaces: HTTP {} - {}",
                status,
                error_text
            ));
        }

        let response_data: serde_json::Value = response.json().await?;
        let empty_vec = vec![];
        let spaces = response_data
            .get("results")
            .and_then(|r| r.as_array())
            .unwrap_or(&empty_vec);

        let space_keys: Vec<String> = spaces
            .iter()
            .filter_map(|s| s.get("key").and_then(|k| k.as_str().map(String::from)))
            .collect();

        debug!("Found {} accessible Confluence spaces", space_keys.len());
        Ok(space_keys)
    }
}
