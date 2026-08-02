use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;
use chrono::{DateTime, NaiveDate, NaiveDateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

pub type DarwinboxConnectorState = JsonValue;

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct DarwinboxCheckpoint {
    pub schema_version: u16,
    #[serde(default)]
    pub synced_person_emails: BTreeSet<String>,
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
    // Extended workplace-directory fields (reviewed, safe to index)
    #[serde(default)]
    pub cost_center: Option<String>,
    #[serde(default)]
    pub office_country: Option<String>,
    #[serde(default)]
    pub grade: Option<String>,
    #[serde(default)]
    pub band: Option<String>,
    #[serde(default)]
    pub confirmation_status: Option<String>,
    #[serde(default, rename = "date_of_joining")]
    pub date_of_joining: Option<String>,
    #[serde(default, rename = "date_of_exit")]
    pub date_of_exit: Option<String>,
    // Extended workplace-directory fields (reviewed, safe to index)
    #[serde(default, alias = "mobile_number")]
    pub personal_mobile_no: Option<String>,
    #[serde(default)]
    pub employee_status: Option<String>,
    #[serde(default)]
    pub top_department: Option<String>,
}

/// Alias for backward compatibility during migration.
pub type EmployeeRecord = EmployeeWireRecord;

#[derive(Debug, Clone, Deserialize)]
pub struct EmployeeDataResponse {
    pub status: Option<i32>,
    pub message: Option<String>,
    pub employee_data: Vec<EmployeeRecord>,
}

impl EmployeeDataResponse {
    pub fn to_person_sync_records(
        &self,
        fields: &[crate::config::EmployeeField],
        include: impl Fn(&EmployeeRecord) -> bool,
    ) -> Result<Vec<omni_connector_sdk::PersonSyncRecord>> {
        self.employee_data
            .iter()
            .filter(|employee| include(employee))
            .map(|employee| {
                let employee_id = employee.employee_id.as_deref().unwrap_or("<missing>");
                if fields.contains(&crate::config::EmployeeField::EmploymentDates) {
                    for (label, value) in [
                        ("date_of_joining", employee.date_of_joining.as_deref()),
                        ("date_of_exit", employee.date_of_exit.as_deref()),
                    ] {
                        if value.is_some() && normalize_darwinbox_date(value).is_none() {
                            return Err(anyhow::anyhow!(
                                "Darwinbox employee {employee_id} has invalid {label}"
                            ));
                        }
                    }
                }
                if employee.latest_modified_any_attribute.is_some()
                    && normalize_darwinbox_timestamp(
                        employee.latest_modified_any_attribute.as_deref(),
                    )
                    .is_none()
                {
                    return Err(anyhow::anyhow!(
                        "Darwinbox employee {employee_id} has invalid latest_modified_any_attribute"
                    ));
                }
                employee.to_person_sync_record(fields).ok_or_else(|| {
                    anyhow::anyhow!(
                        "Darwinbox employee {employee_id} is missing employee_id or company_email"
                    )
                })
            })
            .collect()
    }
}

/// Response envelope for `POST /leavesactionapi/holidaylist`.
///
/// Shape verified live against a production tenant: holidays are returned
/// under the top-level `holidays` key. `holidays` and each item's `name`/
/// `date` are required — a response without them is a shape mismatch and
/// fails the module loudly.
#[derive(Debug, Clone, Deserialize)]
pub struct HolidayListResponse {
    #[serde(default)]
    pub status: Option<i32>,
    pub holidays: Vec<HolidayItem>,
    #[serde(default)]
    pub errors: Vec<serde_json::Value>,
    #[serde(default)]
    pub message: Option<String>,
}

/// One holiday entry as returned by the Darwinbox holiday-list API.
/// All fields are strings on the verified tenant; optional fields tolerate
/// absence with `#[serde(default)]`.
#[derive(Debug, Clone, Deserialize)]
pub struct HolidayItem {
    #[serde(default)]
    pub id: Option<String>,
    pub name: String,
    pub date: String,
    #[serde(default)]
    pub year: Option<String>,
    #[serde(default)]
    pub holiday_repeats: Option<String>,
    #[serde(default)]
    pub is_optional: Option<String>,
    #[serde(default)]
    pub is_national: Option<String>,
}

fn normalize_darwinbox_date(value: Option<&str>) -> Option<String> {
    let value = value?.trim();
    if value.is_empty() {
        return None;
    }
    if let Ok(timestamp) = DateTime::parse_from_rfc3339(value) {
        return Some(timestamp.date_naive().format("%Y-%m-%d").to_string());
    }
    for format in [
        "%d-%m-%Y %H:%M:%S",
        "%d-%b-%Y %H:%M:%S",
        "%Y-%m-%d %H:%M:%S",
    ] {
        if let Ok(timestamp) = NaiveDateTime::parse_from_str(value, format) {
            return Some(timestamp.date().format("%Y-%m-%d").to_string());
        }
    }
    for format in [
        "%Y-%m-%d", "%d-%m-%Y", "%d/%m/%Y", "%Y/%m/%d", "%d %b %Y", "%d-%b-%Y",
    ] {
        if let Ok(date) = NaiveDate::parse_from_str(value, format) {
            return Some(date.format("%Y-%m-%d").to_string());
        }
    }
    None
}

fn normalize_darwinbox_timestamp(value: Option<&str>) -> Option<String> {
    let value = value?.trim();
    if value.is_empty() {
        return None;
    }
    if let Ok(timestamp) = DateTime::parse_from_rfc3339(value) {
        return Some(
            timestamp
                .with_timezone(&Utc)
                .to_rfc3339_opts(SecondsFormat::Secs, true),
        );
    }
    for format in [
        "%d-%m-%Y %H:%M:%S",
        "%d-%b-%Y %H:%M:%S",
        "%Y-%m-%d %H:%M:%S",
    ] {
        if let Ok(timestamp) = NaiveDateTime::parse_from_str(value, format) {
            return Some(
                timestamp
                    .and_utc()
                    .to_rfc3339_opts(SecondsFormat::Secs, true),
            );
        }
    }
    for format in [
        "%Y-%m-%d", "%d-%m-%Y", "%d/%m/%Y", "%Y/%m/%d", "%d %b %Y", "%d-%b-%Y",
    ] {
        if let Ok(date) = NaiveDate::parse_from_str(value, format) {
            return Some(
                date.and_hms_opt(0, 0, 0)?
                    .and_utc()
                    .to_rfc3339_opts(SecondsFormat::Secs, true),
            );
        }
    }
    None
}

/// Map a Darwinbox `employee_status` value to an active flag: only an
/// explicit `Active` counts as active; anything else (`Inactive`,
/// `Resigned`, ...) maps to inactive. Empty values are normalized to `None`
/// by the caller.
fn derive_is_active(employee_status: &str) -> bool {
    employee_status.trim().eq_ignore_ascii_case("Active")
}

impl EmployeeRecord {
    pub fn external_id(&self) -> Option<String> {
        self.employee_id
            .as_deref()
            .filter(|id| !id.is_empty())
            .map(|id| format!("darwinbox:employee:{id}"))
    }

    /// Convert to a PersonSync record while honoring the administrator's
    /// explicit organization-visible field selection.
    pub fn to_person_sync_record(
        &self,
        fields: &[crate::config::EmployeeField],
    ) -> Option<omni_connector_sdk::PersonSyncRecord> {
        use crate::config::EmployeeField;

        let external_id = self.employee_id.as_deref()?.trim();
        if external_id.is_empty() {
            return None;
        }
        let selected = |field: EmployeeField| fields.contains(&field);
        let include_name = selected(EmployeeField::Name);
        let email = selected(EmployeeField::CompanyEmail)
            .then(|| self.company_email_id.as_deref())
            .flatten()?
            .trim()
            .to_lowercase();
        if email.is_empty() {
            return None;
        }
        Some(omni_connector_sdk::PersonSyncRecord {
            external_id: external_id.to_string(),
            email,
            display_name: include_name.then(|| self.display_name()),
            given_name: include_name.then(|| self.first_name.clone()).flatten(),
            middle_name: include_name.then(|| self.middle_name.clone()).flatten(),
            surname: include_name.then(|| self.last_name.clone()).flatten(),
            job_title: selected(EmployeeField::Designation)
                .then(|| self.designation_name.clone())
                .flatten(),
            department: selected(EmployeeField::Department)
                .then(|| self.department_name.clone())
                .flatten(),
            division: None,
            company_name: None,
            office_location: selected(EmployeeField::OfficeLocation)
                .then(|| self.office_area.clone())
                .flatten(),
            work_country: selected(EmployeeField::WorkCountry)
                .then(|| self.office_country.clone())
                .flatten(),
            employee_id: selected(EmployeeField::EmployeeId)
                .then(|| self.employee_id.clone())
                .flatten(),
            employee_type: selected(EmployeeField::EmployeeType)
                .then(|| self.employee_type.clone())
                .flatten(),
            cost_center: selected(EmployeeField::CostCenter)
                .then(|| self.cost_center.clone())
                .flatten(),
            grade: selected(EmployeeField::Grade)
                .then(|| self.grade.clone())
                .flatten(),
            band: selected(EmployeeField::Band)
                .then(|| self.band.clone())
                .flatten(),
            confirmation_status: selected(EmployeeField::ConfirmationStatus)
                .then(|| self.confirmation_status.clone())
                .flatten(),
            employment_start_date: selected(EmployeeField::EmploymentDates)
                .then(|| normalize_darwinbox_date(self.date_of_joining.as_deref()))
                .flatten(),
            employment_end_date: selected(EmployeeField::EmploymentDates)
                .then(|| normalize_darwinbox_date(self.date_of_exit.as_deref()))
                .flatten(),
            phone: selected(EmployeeField::ContactNumber)
                .then(|| self.personal_mobile_no.as_deref())
                .flatten()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string),
            top_department: selected(EmployeeField::TopDepartment)
                .then(|| self.top_department.as_deref())
                .flatten()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string),
            is_active: selected(EmployeeField::EmploymentStatus)
                .then(|| self.employee_status.as_deref())
                .flatten()
                .map(str::trim)
                .filter(|status| !status.is_empty())
                .map(derive_is_active),
            manager_external_id: selected(EmployeeField::ManagerEmployeeId)
                .then(|| {
                    self.direct_manager_employee_id
                        .as_deref()
                        .map(str::trim)
                        .filter(|id| !id.is_empty())
                        .map(ToString::to_string)
                })
                .flatten(),
            source_updated_at: normalize_darwinbox_timestamp(
                self.latest_modified_any_attribute.as_deref(),
            ),
        })
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

    #[allow(dead_code)]
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
                _ => {}
            }
        }
        lines.join("\n")
    }

    /* Legacy employee document construction is intentionally removed from sync.
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
    */
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::EmployeeField;

    #[test]
    fn normalizes_documented_darwinbox_dates() {
        assert_eq!(
            normalize_darwinbox_date(Some("31-07-2026")),
            Some("2026-07-31".into())
        );
        assert_eq!(
            normalize_darwinbox_date(Some("2026-07-31T10:20:30+05:30")),
            Some("2026-07-31".into())
        );
        assert_eq!(
            normalize_darwinbox_date(Some("24-Aug-2020")),
            Some("2020-08-24".into())
        );
        for value in [
            "31-07-2026 10:20:30",
            "31-Jul-2026 10:20:30",
            "2026-07-31 10:20:30",
        ] {
            assert_eq!(
                normalize_darwinbox_date(Some(value)),
                Some("2026-07-31".into())
            );
        }
        assert_eq!(normalize_darwinbox_date(Some("not-a-date")), None);
    }

    #[test]
    fn preserves_provider_modification_instants() {
        assert_eq!(
            normalize_darwinbox_timestamp(Some("2026-07-31T10:20:30+05:30")),
            Some("2026-07-31T04:50:30Z".into())
        );
        for value in [
            "31-07-2026 10:20:30",
            "31-Jul-2026 10:20:30",
            "2026-07-31 10:20:30",
        ] {
            assert_eq!(
                normalize_darwinbox_timestamp(Some(value)),
                Some("2026-07-31T10:20:30Z".into())
            );
        }
        assert_eq!(
            normalize_darwinbox_timestamp(Some("24-Aug-2020")),
            Some("2020-08-24T00:00:00Z".into())
        );
        assert_eq!(normalize_darwinbox_timestamp(Some("not-a-date")), None);
    }

    #[test]
    fn v4_checkpoint_persists_synced_person_emails() {
        let checkpoint = DarwinboxCheckpoint {
            schema_version: 4,
            synced_person_emails: ["ada@example.com".into()].into_iter().collect(),
            ..Default::default()
        };
        let serialized = serde_json::to_value(checkpoint).unwrap();
        assert_eq!(serialized["synced_person_emails"][0], "ada@example.com");
    }

    #[test]
    fn employee_response_requires_an_explicit_employee_data_array() {
        assert!(
            serde_json::from_value::<EmployeeDataResponse>(serde_json::json!({"status": 1}))
                .is_err()
        );
        let response: EmployeeDataResponse = serde_json::from_value(serde_json::json!({
            "status": 1,
            "employee_data": []
        }))
        .unwrap();
        assert!(response.employee_data.is_empty());
    }

    #[test]
    fn employee_wire_record_parses_contact_status_and_top_department() {
        let response: EmployeeDataResponse = serde_json::from_value(serde_json::json!({
            "status": 1,
            "employee_data": [{
                "employee_id": "E-1",
                "company_email_id": "ada@example.com",
                "personal_mobile_no": "  +91-98765-43210 ",
                "employee_status": "Active",
                "top_department": "People",
                "future_unknown_field": {"nested": true}
            }]
        }))
        .unwrap();
        let employee = &response.employee_data[0];
        assert_eq!(
            employee.personal_mobile_no.as_deref(),
            Some("  +91-98765-43210 ")
        );
        assert_eq!(employee.employee_status.as_deref(), Some("Active"));
        assert_eq!(employee.top_department.as_deref(), Some("People"));
    }

    #[test]
    fn employee_wire_record_accepts_mobile_number_alias() {
        let response: EmployeeDataResponse = serde_json::from_value(serde_json::json!({
            "status": 1,
            "employee_data": [{
                "employee_id": "E-1",
                "company_email_id": "ada@example.com",
                "mobile_number": "9876543210"
            }]
        }))
        .unwrap();
        assert_eq!(
            response.employee_data[0].personal_mobile_no.as_deref(),
            Some("9876543210")
        );
    }

    #[test]
    fn people_projection_maps_contact_status_and_top_department() {
        let response = EmployeeDataResponse {
            status: Some(1),
            message: None,
            employee_data: vec![EmployeeRecord {
                employee_id: Some("E-1".into()),
                first_name: Some("Ada".into()),
                company_email_id: Some("ada@example.com".into()),
                personal_mobile_no: Some("  +91-98765-43210 ".into()),
                employee_status: Some("Active".into()),
                top_department: Some("People".into()),
                ..Default::default()
            }],
        };
        let records = response
            .to_person_sync_records(&crate::config::APPROVED_EMPLOYEE_FIELDS, |_| true)
            .unwrap();
        assert_eq!(records[0].phone.as_deref(), Some("+91-98765-43210"));
        assert_eq!(records[0].top_department.as_deref(), Some("People"));
        assert_eq!(records[0].is_active, Some(true));
    }

    #[test]
    fn people_projection_derives_inactive_status_and_normalizes_empties() {
        let response = EmployeeDataResponse {
            status: Some(1),
            message: None,
            employee_data: vec![
                EmployeeRecord {
                    employee_id: Some("E-1".into()),
                    company_email_id: Some("a@example.com".into()),
                    employee_status: Some("Resigned".into()),
                    personal_mobile_no: Some("   ".into()),
                    top_department: Some("".into()),
                    ..Default::default()
                },
                EmployeeRecord {
                    employee_id: Some("E-2".into()),
                    company_email_id: Some("b@example.com".into()),
                    ..Default::default()
                },
            ],
        };
        let records = response
            .to_person_sync_records(&crate::config::APPROVED_EMPLOYEE_FIELDS, |_| true)
            .unwrap();
        assert_eq!(records[0].is_active, Some(false));
        assert_eq!(records[0].phone, None);
        assert_eq!(records[0].top_department, None);
        // No employee_status present -> leave the row's value untouched.
        assert_eq!(records[1].is_active, None);
    }

    #[test]
    fn people_projection_honors_selected_fields_and_rejects_bad_dates() {
        let response = EmployeeDataResponse {
            status: Some(1),
            message: None,
            employee_data: vec![EmployeeRecord {
                employee_id: Some("E-1".into()),
                first_name: Some("Ada".into()),
                company_email_id: Some("ada@example.com".into()),
                date_of_joining: Some("31/07/2026".into()),
                latest_modified_any_attribute: Some("31-Jul-2026 10:20:30".into()),
                ..Default::default()
            }],
        };
        let records = response
            .to_person_sync_records(
                &[EmployeeField::EmployeeId, EmployeeField::CompanyEmail],
                |_| true,
            )
            .unwrap();
        assert_eq!(records[0].employee_id.as_deref(), Some("E-1"));
        assert_eq!(records[0].email, "ada@example.com");
        assert_eq!(records[0].given_name, None);
        assert_eq!(records[0].employment_start_date, None);
        assert_eq!(
            records[0].source_updated_at.as_deref(),
            Some("2026-07-31T10:20:30Z")
        );

        let mut invalid = response;
        invalid.employee_data[0].date_of_joining = Some("bad".into());
        assert!(
            invalid
                .to_person_sync_records(&[EmployeeField::EmploymentDates], |_| true)
                .is_err()
        );
    }

    #[test]
    fn holiday_response_deserializes_the_verified_live_shape() {
        let response: HolidayListResponse = serde_json::from_value(serde_json::json!({
            "status": 1,
            "holidays": [{
                "id": "a68f996eb7bec5",
                "name": "New Year's Holiday",
                "date": "2026-01-01",
                "year": "2026",
                "holiday_repeats": "No",
                "is_optional": "No",
                "is_national": "No"
            }],
            "errors": [],
            "message": "Successfully Loaded All Holidays"
        }))
        .unwrap();
        assert_eq!(response.status, Some(1));
        assert_eq!(response.holidays.len(), 1);
        let holiday = &response.holidays[0];
        assert_eq!(holiday.id.as_deref(), Some("a68f996eb7bec5"));
        assert_eq!(holiday.name, "New Year's Holiday");
        assert_eq!(holiday.date, "2026-01-01");
        assert_eq!(holiday.year.as_deref(), Some("2026"));
        assert_eq!(holiday.holiday_repeats.as_deref(), Some("No"));
        assert_eq!(holiday.is_optional.as_deref(), Some("No"));
        assert_eq!(holiday.is_national.as_deref(), Some("No"));
    }

    #[test]
    fn holiday_response_requires_name_and_date_on_every_item() {
        // A missing `holidays` key is a shape mismatch and must fail loudly.
        assert!(
            serde_json::from_value::<HolidayListResponse>(serde_json::json!({
                "status": 1,
                "result": []
            }))
            .is_err()
        );
        // A non-array `holidays` value is likewise a shape mismatch.
        assert!(
            serde_json::from_value::<HolidayListResponse>(serde_json::json!({
                "holidays": "oops"
            }))
            .is_err()
        );
        // An item without `date` is a shape mismatch and must fail loudly.
        assert!(
            serde_json::from_value::<HolidayListResponse>(serde_json::json!({
                "status": 1,
                "holidays": [{"id": "a68f996eb7bec5", "name": "Independence Day"}]
            }))
            .is_err()
        );
        // An item without `name` is likewise a shape mismatch.
        assert!(
            serde_json::from_value::<HolidayListResponse>(serde_json::json!({
                "status": 1,
                "holidays": [{"id": "a68f996eb7bec5", "date": "2026-08-15"}]
            }))
            .is_err()
        );
    }

    #[test]
    fn holiday_response_tolerates_absent_envelope_and_optional_fields() {
        // `errors`/`message`/optional item fields may be absent; defaults apply.
        let response: HolidayListResponse = serde_json::from_value(serde_json::json!({
            "status": 1,
            "holidays": [{"name": "Independence Day", "date": "2026-08-15"}]
        }))
        .unwrap();
        assert!(response.errors.is_empty());
        assert_eq!(response.message, None);
        let holiday = &response.holidays[0];
        assert_eq!(holiday.id, None);
        assert_eq!(holiday.year, None);
        assert_eq!(holiday.holiday_repeats, None);
        assert_eq!(holiday.is_optional, None);
        assert_eq!(holiday.is_national, None);
    }
}
