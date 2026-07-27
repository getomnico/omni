use serde::{Deserialize, Serialize};
use url::Url;

fn default_read_only() -> bool {
    true
}

fn default_max_batch_size() -> usize {
    1
}

/// Safe pre-approved employee fields that may be indexed.
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
}

/// How employee scope is determined for indexing.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum EmployeeScope {
    /// Index all employees returned by Darwinbox. This must be explicitly set.
    All,
    /// Index only explicitly listed employees/departments.
    Include {
        #[serde(default)]
        employee_ids: Vec<String>,
        #[serde(default)]
        employee_emails: Vec<String>,
        #[serde(default)]
        departments: Vec<String>,
    },
}

impl EmployeeScope {
    /// Returns true if the given employee record is within scope.
    pub fn includes(&self, employee: &crate::models::EmployeeRecord) -> bool {
        match self {
            Self::All => true,
            Self::Include {
                employee_ids,
                employee_emails,
                departments,
            } => {
                // Empty include means deny everything (fail closed)
                if employee_ids.is_empty() && employee_emails.is_empty() && departments.is_empty() {
                    return false;
                }
                if let Some(id) = &employee.employee_id {
                    if employee_ids.iter().any(|eid| eid.eq_ignore_ascii_case(id)) {
                        return true;
                    }
                }
                if let Some(email) = &employee.company_email_id {
                    if employee_emails
                        .iter()
                        .any(|e| e.eq_ignore_ascii_case(email.trim()))
                    {
                        return true;
                    }
                }
                if let Some(dept) = &employee.department_name {
                    if departments.iter().any(|d| d.eq_ignore_ascii_case(dept)) {
                        return true;
                    }
                }
                false
            }
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DarwinboxSourceConfig {
    pub base_url: String,
    #[serde(default = "default_read_only")]
    pub read_only: bool,
    #[serde(default)]
    pub default_timezone: Option<String>,
    #[serde(default)]
    pub employee_scope: Option<EmployeeScope>,
    #[serde(default)]
    pub employee_fields: Vec<EmployeeField>,
    #[serde(default)]
    pub sync_modules: DarwinboxSyncModules,
    #[serde(default)]
    pub action_modules: DarwinboxActionModules,
    #[serde(default)]
    pub authorization: DarwinboxAuthorizationConfig,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DarwinboxSyncModules {
    #[serde(default)]
    pub employee_directory: bool,
    #[serde(default)]
    pub deleted_employees: bool,
    /// Controls department master indexing. Individual flags below replace the
    /// legacy `org_masters` flag; all default to disabled.
    #[serde(default)]
    pub departments: bool,
    #[serde(default)]
    pub designations: bool,
    #[serde(default)]
    pub office_locations: bool,
    #[serde(default)]
    pub business_units: bool,
    #[serde(default)]
    pub divisions: bool,
    #[serde(default)]
    pub cost_centers: bool,
    #[serde(default)]
    pub group_companies: bool,
    #[serde(default)]
    pub positions: bool,
    #[serde(default)]
    pub holidays: bool,
    #[serde(default)]
    pub ats_jobs: bool,
}

impl Default for DarwinboxSyncModules {
    fn default() -> Self {
        Self {
            employee_directory: false,
            deleted_employees: false,
            departments: false,
            designations: false,
            office_locations: false,
            business_units: false,
            divisions: false,
            cost_centers: false,
            group_companies: false,
            positions: false,
            holidays: false,
            ats_jobs: false,
        }
    }
}

impl DarwinboxSyncModules {
    /// Returns true if any org master entity type is enabled.
    pub fn has_any_org_master(&self) -> bool {
        self.departments
            || self.designations
            || self.office_locations
            || self.business_units
            || self.divisions
            || self.cost_centers
            || self.group_companies
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DarwinboxActionModules {
    #[serde(default)]
    pub employee_self_service: bool,
    #[serde(default)]
    pub manager_workflows: bool,
    #[serde(default)]
    pub hr_operations: bool,
    #[serde(default)]
    pub ats: bool,
    #[serde(default)]
    pub reports: bool,
}

impl Default for DarwinboxActionModules {
    fn default() -> Self {
        Self {
            employee_self_service: false,
            manager_workflows: false,
            hr_operations: false,
            ats: false,
            reports: false,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DarwinboxAuthorizationConfig {
    #[serde(default)]
    pub use_darwinbox_permissions: Option<bool>,
    #[serde(default)]
    pub actions_enabled: bool,
    #[serde(default)]
    pub write_acknowledged: bool,
    #[serde(default)]
    pub participant_emails: Vec<String>,
    #[serde(default)]
    pub allowed_actions: Vec<String>,
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
            use_darwinbox_permissions: None,
            actions_enabled: false,
            write_acknowledged: false,
            participant_emails: Vec::new(),
            allowed_actions: Vec::new(),
            allowed_report_ids: Vec::new(),
            recruiter_emails: Vec::new(),
            max_batch_size: 1,
        }
    }
}

impl DarwinboxSourceConfig {
    pub fn is_employee_in_scope(&self, employee: &crate::models::EmployeeRecord) -> bool {
        self.employee_scope
            .as_ref()
            .map(|scope| scope.includes(employee))
            .unwrap_or(false)
    }

    pub fn is_action_participant(&self, email: &str) -> bool {
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

        if self.authorization.use_darwinbox_permissions.is_some() {
            errors.push("use_darwinbox_permissions is unsupported; remove it".to_string());
        }
        if self.sync_modules.employee_directory {
            if self.employee_scope.is_none() {
                errors.push(
                    "employee_scope is required when employee_directory is enabled".to_string(),
                );
            }
            if self.employee_fields.is_empty() {
                errors.push("employee_fields must contain at least one approved field".to_string());
            }
        }
        if self.sync_modules.has_any_org_master()
            || self.sync_modules.positions
            || self.sync_modules.holidays
            || self.sync_modules.ats_jobs
        {
            errors.push(
                "organization masters, positions, holidays, and ATS job sync are unavailable until typed Darwinbox response contracts are configured"
                    .to_string(),
            );
        }
        if self.authorization.max_batch_size == 0 || self.authorization.max_batch_size > 20 {
            errors.push("max_batch_size must be between 1 and 20".to_string());
        }

        let actions_requested = self.action_modules.employee_self_service
            || self.action_modules.manager_workflows
            || self.action_modules.hr_operations
            || self.action_modules.ats
            || self.action_modules.reports;
        if actions_requested {
            if !self.authorization.actions_enabled {
                errors.push("action modules require actions_enabled=true".to_string());
            }
            if self.authorization.participant_emails.is_empty()
                || self.authorization.allowed_actions.is_empty()
            {
                errors.push("actions require participant_emails and allowed_actions".to_string());
            }
            if self.action_modules.manager_workflows {
                errors.push(
                    "manager workflows require an explicit target employee scope".to_string(),
                );
            }
        }
        if !self.read_only {
            if !self.authorization.write_acknowledged {
                errors.push("disabling read_only requires write_acknowledged=true".to_string());
            }
            if !self.authorization.allowed_actions.iter().any(|action| {
                crate::actions::find_action_policy(action).is_some_and(|policy| policy.is_write)
            }) {
                errors.push(
                    "disabling read_only requires at least one allowlisted write action"
                        .to_string(),
                );
            }
        }
        // High-risk generic HR/ATS payloads remain unavailable until
        // action-specific provider contracts have been reviewed.
        if self.action_modules.hr_operations || self.action_modules.ats {
            errors.push("HR and ATS actions are not implemented".to_string());
        }
        const UNTYPED_ACTIONS: &[&str] = &[
            "regularize_my_attendance",
            "add_pending_employee",
            "activate_pending_employee",
            "update_employee_record",
            "update_employment_details",
            "deactivate_employee",
            "reactivate_employee",
            "upload_employee_document",
            "fetch_employee_history",
            "list_jobs",
            "get_job_detail",
            "get_candidates",
            "tag_candidate",
            "reject_candidate",
            "create_requisition",
            "archive_requisition",
        ];
        for action in &self.authorization.allowed_actions {
            if crate::actions::find_action_policy(action).is_none() {
                errors.push(format!("unknown Darwinbox action '{action}'"));
            }
            if UNTYPED_ACTIONS.contains(&action.as_str()) {
                errors.push(format!("action '{action}' is not implemented"));
            }
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
    config: &DarwinboxSourceConfig,
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
