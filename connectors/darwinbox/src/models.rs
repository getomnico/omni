use std::collections::{BTreeMap, BTreeSet};

use omni_connector_sdk::{ConnectorEvent, DocumentMetadata, DocumentPermissions};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as JsonValue};

pub type DarwinboxConnectorState = JsonValue;

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct DarwinboxCheckpoint {
    pub schema_version: u16,
    #[serde(default)]
    pub policy_fingerprint: Option<String>,
    #[serde(default)]
    pub indexed_document_ids: BTreeSet<String>,
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

/// Wire-level employee record from Darwinbox. Only known safe fields are
/// captured; unknown fields are silently ignored via missing-field defaults.
/// This type intentionally does NOT use `deny_unknown_fields` so provider
/// response extensions do not break deserialization.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct EmployeeWireRecord {
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
}

/// Alias for backward compatibility during migration.
pub type EmployeeRecord = EmployeeWireRecord;

#[derive(Debug, Clone, Deserialize)]
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

    /// Convenience constructor for content from selected fields only.
    pub fn content_filtered(&self, fields: &[crate::config::EmployeeField]) -> String {
        let mut lines = Vec::new();
        for field in fields {
            match field {
                crate::config::EmployeeField::Name => {
                    lines.push(self.display_name());
                }
                crate::config::EmployeeField::EmployeeId => {
                    if let Some(id) = &self.employee_id {
                        lines.push(format!("Employee ID: {id}"));
                    }
                }
                crate::config::EmployeeField::CompanyEmail => {
                    if let Some(email) = &self.company_email_id {
                        lines.push(format!("Email: {email}"));
                    }
                }
                crate::config::EmployeeField::Department => {
                    if let Some(dept) = &self.department_name {
                        lines.push(format!("Department: {dept}"));
                    }
                }
                crate::config::EmployeeField::Designation => {
                    if let Some(desig) = &self.designation_name {
                        lines.push(format!("Designation: {desig}"));
                    }
                }
                crate::config::EmployeeField::OfficeLocation => {
                    if let Some(loc) = &self.office_area {
                        lines.push(format!("Location: {loc}"));
                    }
                }
                crate::config::EmployeeField::ManagerEmployeeId => {
                    if let Some(mgr) = &self.direct_manager_employee_id {
                        lines.push(format!("Manager Employee ID: {mgr}"));
                    }
                }
                crate::config::EmployeeField::EmployeeType => {
                    if let Some(etype) = &self.employee_type {
                        lines.push(format!("Employee Type: {etype}"));
                    }
                }
            }
        }
        lines.join("\n")
    }

    /// Build a document-create event with the given permissions.
    pub fn to_event_with_permissions(
        &self,
        sync_run_id: String,
        source_id: String,
        content_id: String,
        content_size: usize,
        fields: &[crate::config::EmployeeField],
        permissions: DocumentPermissions,
    ) -> Option<ConnectorEvent> {
        let document_id = self.external_id()?;
        let title = if fields.contains(&crate::config::EmployeeField::Name) {
            self.display_name()
        } else {
            "Darwinbox employee".to_string()
        };
        let metadata = DocumentMetadata {
            title: Some(title),
            author: fields
                .contains(&crate::config::EmployeeField::CompanyEmail)
                .then(|| self.company_email_id.clone())
                .flatten(),
            created_at: None,
            updated_at: None,
            content_type: Some("employee_profile".to_string()),
            mime_type: Some("text/markdown".to_string()),
            size: Some(content_size.to_string()),
            url: None,
            path: None,
            extra: None,
        };
        let mut attributes =
            std::collections::HashMap::from([("source_type".to_string(), json!("darwinbox"))]);
        for field in fields {
            match field {
                crate::config::EmployeeField::EmployeeId => {
                    attributes.insert("employee_id".to_string(), json!(self.employee_id));
                }
                crate::config::EmployeeField::CompanyEmail => {
                    attributes.insert("email".to_string(), json!(self.company_email_id));
                }
                crate::config::EmployeeField::Department => {
                    attributes.insert("department".to_string(), json!(self.department_name));
                }
                crate::config::EmployeeField::Designation => {
                    attributes.insert("designation".to_string(), json!(self.designation_name));
                }
                crate::config::EmployeeField::OfficeLocation => {
                    attributes.insert("location".to_string(), json!(self.office_area));
                }
                crate::config::EmployeeField::ManagerEmployeeId => {
                    attributes.insert(
                        "manager_employee_id".to_string(),
                        json!(self.direct_manager_employee_id),
                    );
                }
                crate::config::EmployeeField::EmployeeType => {
                    attributes.insert("employee_type".to_string(), json!(self.employee_type));
                }
                crate::config::EmployeeField::Name => {}
            }
        }
        Some(ConnectorEvent::DocumentCreated {
            sync_run_id,
            source_id,
            document_id,
            content_id,
            metadata,
            permissions,
            attributes: Some(attributes),
        })
    }
}
