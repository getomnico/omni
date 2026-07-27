//! Typed mappers from Darwinbox provider models to Omni document events.
//! Each entity type has its own mapper so raw provider fields are never
//! serialized into indexed content, metadata, attributes, or logs.

use omni_connector_sdk::ConnectorEvent;
use serde_json::Value as JsonValue;

use crate::config::{self, DarwinboxSourceConfig};
use crate::models::EmployeeRecord;

/// Map a Darwinbox employee record to a document-create event with the
/// appropriate filtered content and ACL.
pub fn employee_to_event(
    employee: &EmployeeRecord,
    sync_run_id: String,
    source_id: String,
    content_id: String,
    config: &DarwinboxSourceConfig,
) -> Option<ConnectorEvent> {
    let permissions = config::document_permissions(
        "employee_profile",
        config,
        &source_id,
        employee.company_email_id.as_deref(),
    );
    let content = employee.content_filtered(&config.employee_fields);
    employee.to_event_with_permissions(
        sync_run_id,
        source_id,
        content_id,
        content.len(),
        &config.employee_fields,
        permissions,
    )
}

/// Map a generic Darwinbox entity to a document-create event with safe
/// field projection and non-public ACL.

/// Safely format an org master item's title and content from known fields only.
pub fn format_org_master_item<'a>(
    item: &'a JsonValue,
    _content_type: &'a str,
) -> (&'a str, String) {
    let title = item
        .get("name")
        .and_then(JsonValue::as_str)
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| {
            item.get("code")
                .and_then(JsonValue::as_str)
                .unwrap_or("unknown")
        });

    let code = item
        .get("code")
        .and_then(JsonValue::as_str)
        .unwrap_or_default();
    let description = item
        .get("description")
        .and_then(JsonValue::as_str)
        .unwrap_or_default();
    let status = item
        .get("status")
        .and_then(JsonValue::as_str)
        .unwrap_or_default();

    let safe_body = if !description.is_empty() {
        format!("# {title}\n\n- Code: {code}\n- Description: {description}\n- Status: {status}")
    } else {
        format!("# {title}\n\n- Code: {code}\n- Status: {status}")
    };

    (title, safe_body)
}

/// Safely format a holiday item from known fields.
pub fn format_holiday_item(item: &JsonValue) -> (String, String) {
    let name = item
        .get("holiday_name")
        .and_then(JsonValue::as_str)
        .unwrap_or("Holiday");
    let date = item
        .get("holiday_date")
        .and_then(JsonValue::as_str)
        .unwrap_or_default();
    let description = item
        .get("description")
        .and_then(JsonValue::as_str)
        .unwrap_or_default();

    let safe_body = if description.is_empty() {
        format!("# {name}\n\n- Date: {date}")
    } else {
        format!("# {name}\n\n- Date: {date}\n- Description: {description}")
    };
    (name.to_string(), safe_body)
}

/// Safely format a position item from known fields.
pub fn format_position_item(item: &JsonValue) -> (String, String) {
    let title = item
        .get("name")
        .and_then(JsonValue::as_str)
        .unwrap_or("Position");
    let code = item
        .get("position_code")
        .and_then(JsonValue::as_str)
        .unwrap_or("");
    let status = item.get("status").and_then(JsonValue::as_str).unwrap_or("");

    let safe_body = if !status.is_empty() {
        format!("# {title}\n\n- Code: {code}\n- Status: {status}")
    } else {
        format!("# {title}\n\n- Code: {code}")
    };
    (title.to_string(), safe_body)
}

/// Safely format an ATS job from known fields.
pub fn format_ats_job_item(item: &JsonValue) -> (String, String) {
    let title = item
        .get("job_title")
        .and_then(JsonValue::as_str)
        .unwrap_or("Job");
    let job_id = item.get("job_id").and_then(JsonValue::as_str).unwrap_or("");

    let safe_body = format!("# {title}\n\n- Job ID: {job_id}");
    (title.to_string(), safe_body)
}
