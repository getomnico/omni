use std::collections::BTreeMap;

use omni_connector_sdk::{ConnectorEvent, DocumentMetadata, DocumentPermissions};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as JsonValue};

pub type DarwinboxConnectorState = JsonValue;

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct DarwinboxCheckpoint {
    pub schema_version: u16,
    #[serde(default)]
    pub modules: BTreeMap<DarwinboxSyncModuleKey, ModuleCheckpoint>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ModuleCheckpoint {
    #[serde(default)]
    pub watermark_ts: Option<String>,
    #[serde(default)]
    pub in_progress: Option<InProgressCheckpoint>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct InProgressCheckpoint {
    pub unit: DarwinboxSyncUnit,
    #[serde(default)]
    pub page_cursor: Option<String>,
    #[serde(default)]
    pub page_offset: Option<u64>,
    #[serde(default)]
    pub current_year: Option<i32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DarwinboxSyncModuleKey {
    EmployeeDirectory,
    DeletedEmployees,
    OrgMasters,
    PositionMaster,
    Holidays,
    AtsJobs,
    AtsCandidates,
    Reports,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DarwinboxSyncUnit {
    EmployeeDirectory,
    DeletedEmployees,
    OrgMasterDepartments,
    OrgMasterDesignations,
    OrgMasterLocations,
    OrgMasterBusinessUnits,
    OrgMasterDivisions,
    OrgMasterCostCenters,
    OrgMasterGroupCompanies,
    PositionMaster,
    Holidays { year: i32 },
    AtsJobs,
    AtsCandidates,
    Report { report_id: String },
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct EmployeeRecord {
    #[serde(default)]
    pub employee_id: Option<String>,
    #[serde(default)]
    pub first_name: Option<String>,
    #[serde(default)]
    pub middle_name: Option<String>,
    #[serde(default)]
    pub last_name: Option<String>,
    #[serde(default)]
    pub company_email_id: Option<String>,
    #[serde(default)]
    pub department_name: Option<String>,
    #[serde(default)]
    pub designation_name: Option<String>,
    #[serde(default)]
    pub office_area: Option<String>,
    #[serde(default)]
    pub direct_manager_employee_id: Option<String>,
    #[serde(default)]
    pub employee_type: Option<String>,
    #[serde(default)]
    pub latest_modified_any_attribute: Option<String>,
    #[serde(flatten)]
    pub extra: JsonValue,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct EmployeeDataResponse {
    pub status: Option<i32>,
    pub message: Option<String>,
    #[serde(default)]
    pub employee_data: Vec<EmployeeRecord>,
}

impl EmployeeRecord {
    pub fn external_id(&self) -> Option<String> {
        self.employee_id
            .as_deref()
            .filter(|id| !id.is_empty())
            .map(|id| format!("darwinbox:employee:{id}"))
    }

    pub fn display_name(&self) -> String {
        let parts = [
            self.first_name.as_deref(),
            self.middle_name.as_deref(),
            self.last_name.as_deref(),
        ]
        .into_iter()
        .flatten()
        .filter(|part| !part.trim().is_empty())
        .collect::<Vec<_>>();

        if parts.is_empty() {
            self.employee_id
                .clone()
                .unwrap_or_else(|| "Unknown employee".to_string())
        } else {
            parts.join(" ")
        }
    }

    pub fn content(&self) -> String {
        let mut lines = vec![format!("# {}", self.display_name())];
        if let Some(employee_id) = &self.employee_id {
            lines.push(format!("Employee ID: {employee_id}"));
        }
        if let Some(email) = &self.company_email_id {
            lines.push(format!("Email: {email}"));
        }
        if let Some(department) = &self.department_name {
            lines.push(format!("Department: {department}"));
        }
        if let Some(designation) = &self.designation_name {
            lines.push(format!("Designation: {designation}"));
        }
        if let Some(location) = &self.office_area {
            lines.push(format!("Location: {location}"));
        }
        if let Some(manager) = &self.direct_manager_employee_id {
            lines.push(format!("Manager Employee ID: {manager}"));
        }
        lines.join("\n")
    }

    pub fn to_event(
        &self,
        sync_run_id: String,
        source_id: String,
        content_id: String,
    ) -> Option<ConnectorEvent> {
        let document_id = self.external_id()?;
        let title = self.display_name();
        let metadata = DocumentMetadata {
            title: Some(title),
            author: self.company_email_id.clone(),
            created_at: None,
            updated_at: None,
            content_type: Some("employee_profile".to_string()),
            mime_type: Some("text/markdown".to_string()),
            size: Some(self.content().len().to_string()),
            url: None,
            path: None,
            extra: Some(std::collections::HashMap::from([(
                "darwinbox".to_string(),
                json!({ "employee_id": self.employee_id }),
            )])),
        };
        let permissions = DocumentPermissions {
            public: true,
            users: vec![],
            groups: vec![],
        };
        Some(ConnectorEvent::DocumentCreated {
            sync_run_id,
            source_id,
            document_id,
            content_id,
            metadata,
            permissions,
            attributes: Some(std::collections::HashMap::from([
                ("source_type".to_string(), json!("darwinbox")),
                ("employee_id".to_string(), json!(self.employee_id)),
                ("email".to_string(), json!(self.company_email_id)),
                ("department".to_string(), json!(self.department_name)),
                ("designation".to_string(), json!(self.designation_name)),
                ("location".to_string(), json!(self.office_area)),
                (
                    "manager_employee_id".to_string(),
                    json!(self.direct_manager_employee_id),
                ),
                ("employee_type".to_string(), json!(self.employee_type)),
            ])),
        })
    }
}
