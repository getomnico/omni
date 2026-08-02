use serde::{Deserialize, Serialize};
use url::Url;

fn default_read_only() -> bool {
    true
}

fn default_max_batch_size() -> usize {
    1
}

/// Safe pre-approved employee fields that may be projected into People records.
/// The provider dataset key is the access control for what the API returns;
/// this list is the maximum field set Omni will ever write into the directory.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EmployeeField {
    Name,
    EmployeeId,
    CompanyEmail,
    Department,
    Designation,
    OfficeLocation,
    ManagerEmployeeId,
    EmployeeType,
    CostCenter,
    WorkCountry,
    Grade,
    Band,
    ConfirmationStatus,
    EmploymentDates,
}

/// Fixed approved field set projected into People records; not admin-configurable.
pub const APPROVED_EMPLOYEE_FIELDS: &[EmployeeField] = &[
    EmployeeField::Name,
    EmployeeField::EmployeeId,
    EmployeeField::CompanyEmail,
    EmployeeField::Department,
    EmployeeField::Designation,
    EmployeeField::OfficeLocation,
    EmployeeField::ManagerEmployeeId,
    EmployeeField::EmployeeType,
    EmployeeField::CostCenter,
    EmployeeField::WorkCountry,
    EmployeeField::Grade,
    EmployeeField::Band,
    EmployeeField::ConfirmationStatus,
    EmployeeField::EmploymentDates,
];

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DarwinboxSourceConfig {
    pub base_url: String,
    #[serde(default = "default_read_only")]
    pub read_only: bool,
    #[serde(default)]
    pub default_timezone: Option<String>,
    #[serde(default)]
    pub authorization: DarwinboxAuthorizationConfig,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DarwinboxAuthorizationConfig {
    /// "all" (default) makes interactive actions available to every
    /// authenticated user; "allowlist" restricts them to `participant_emails`.
    #[serde(default)]
    pub participant_mode: Option<String>,
    #[serde(default)]
    pub participant_emails: Vec<String>,
    #[serde(default)]
    pub recruiter_emails: Vec<String>,
    #[serde(default)]
    pub allowed_report_ids: Vec<String>,
    #[serde(default = "default_max_batch_size")]
    pub max_batch_size: usize,
}

impl Default for DarwinboxAuthorizationConfig {
    fn default() -> Self {
        Self {
            participant_mode: None,
            participant_emails: Vec::new(),
            recruiter_emails: Vec::new(),
            allowed_report_ids: Vec::new(),
            max_batch_size: 1,
        }
    }
}

impl DarwinboxSourceConfig {
    /// Resolve the effective participant mode. Configs created before
    /// `participant_mode` existed carry only an email allowlist, so a missing
    /// mode is derived from the list: a non-empty allowlist stays restricted
    /// instead of silently opening up to everyone.
    pub fn participant_mode(&self) -> &str {
        match self.authorization.participant_mode.as_deref() {
            Some(mode @ ("all" | "allowlist")) => mode,
            _ => {
                if self.authorization.participant_emails.is_empty() {
                    "all"
                } else {
                    "allowlist"
                }
            }
        }
    }

    pub fn is_action_participant(&self, email: &str) -> bool {
        if self.participant_mode() == "all" {
            return true;
        }
        let email = normalize_email(email);
        normalize_emails(&self.authorization.participant_emails).contains(&email)
    }

    /// Validate the configuration and return an error if it is unsafe or
    /// contradictory.
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();

        if self.base_url.trim().is_empty() {
            errors.push("base_url is required".to_string());
        } else {
            match Url::parse(&self.base_url) {
                Ok(url) => {
                    let is_loopback = url.host_str().is_some_and(|host| {
                        host.eq_ignore_ascii_case("localhost")
                            || host
                                .parse::<std::net::IpAddr>()
                                .is_ok_and(|ip| ip.is_loopback())
                    });
                    if url.scheme() != "https" && !(url.scheme() == "http" && is_loopback) {
                        errors.push("base_url must use HTTPS".to_string());
                    }
                    if !url.username().is_empty() || url.password().is_some() {
                        errors.push("base_url must not include credentials".to_string());
                    }
                }
                Err(_) => errors.push("base_url must be a valid URL".to_string()),
            }
        }

        if self.authorization.max_batch_size == 0 || self.authorization.max_batch_size > 20 {
            errors.push("max_batch_size must be between 1 and 20".to_string());
        }
        if let Some(mode) = self.authorization.participant_mode.as_deref() {
            if !matches!(mode, "all" | "allowlist") {
                errors.push("participant_mode must be 'all' or 'allowlist'".to_string());
            }
        }
        if self.participant_mode() == "allowlist"
            && self.authorization.participant_emails.is_empty()
        {
            errors
                .push("actions restricted to an allowlist require participant_emails".to_string());
        }
        for email in self.authorization.participant_emails.iter() {
            if !email.contains('@') || email.trim().is_empty() {
                errors.push(format!("invalid authorization email: {email}"));
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

/// Normalize email addresses: trim whitespace and lowercase.
pub fn normalize_email(email: &str) -> String {
    email.trim().to_ascii_lowercase()
}

/// Normalize and deduplicate a list of emails.
pub fn normalize_emails(emails: &[String]) -> Vec<String> {
    let mut result: Vec<String> = emails
        .iter()
        .map(|e| normalize_email(e))
        .filter(|e| !e.is_empty() && e.contains('@'))
        .collect();
    result.sort();
    result.dedup();
    result
}

/// Determine document permissions for a given Darwinbox content type and
/// configuration. All documents are non-public; access is granted via
/// direct user emails or provider-scoped groups.
pub fn document_permissions(
    content_type: &str,
    _config: &DarwinboxSourceConfig,
    _source_id: &str,
    employee_self_email: Option<&str>,
) -> omni_connector_sdk::DocumentPermissions {
    match content_type {
        "employee_profile" => {
            let mut users: Vec<String> = Vec::new();
            if let Some(email) = employee_self_email {
                let normalized = normalize_email(email);
                users.push(normalized);
            }
            omni_connector_sdk::DocumentPermissions {
                public: false,
                users,
                groups: vec![],
            }
        }
        "department" | "designation" | "office_location" | "business_unit" | "division"
        | "cost_center" | "group_company" | "holiday" | "position" | "job" | "ats_job" => {
            omni_connector_sdk::DocumentPermissions {
                public: true,
                users: vec![],
                groups: vec![],
            }
        }
        // Unknown/future content types: fail closed
        _ => omni_connector_sdk::DocumentPermissions {
            public: false,
            users: vec![],
            groups: vec![],
        },
    }
}
