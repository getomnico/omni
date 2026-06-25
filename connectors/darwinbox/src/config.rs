use serde::{Deserialize, Serialize};

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DarwinboxSourceConfig {
    pub base_url: String,
    #[serde(default)]
    pub default_timezone: Option<String>,
    #[serde(default)]
    pub sync_modules: DarwinboxSyncModules,
    #[serde(default)]
    pub action_modules: DarwinboxActionModules,
    #[serde(default)]
    pub authorization: DarwinboxAuthorizationConfig,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DarwinboxSyncModules {
    #[serde(default = "default_true")]
    pub employee_directory: bool,
    #[serde(default = "default_true")]
    pub deleted_employees: bool,
    #[serde(default = "default_true")]
    pub org_masters: bool,
    #[serde(default)]
    pub positions: bool,
    #[serde(default = "default_true")]
    pub holidays: bool,
    #[serde(default)]
    pub ats_jobs: bool,
}

impl Default for DarwinboxSyncModules {
    fn default() -> Self {
        Self {
            employee_directory: true,
            deleted_employees: true,
            org_masters: true,
            positions: false,
            holidays: true,
            ats_jobs: false,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DarwinboxActionModules {
    #[serde(default = "default_true")]
    pub employee_self_service: bool,
    #[serde(default = "default_true")]
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
            employee_self_service: true,
            manager_workflows: true,
            hr_operations: false,
            ats: false,
            reports: false,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct DarwinboxAuthorizationConfig {
    #[serde(default = "default_true")]
    pub use_darwinbox_permissions: bool,
    #[serde(default)]
    pub hr_admin_emails: Vec<String>,
    #[serde(default)]
    pub recruiter_emails: Vec<String>,
}
