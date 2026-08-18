//! Typed mappers from Darwinbox provider models to Omni document events.
//! Each entity type has its own mapper so raw provider fields are never
//! serialized into indexed content, metadata, attributes, or logs.

use serde_json::Value as JsonValue;

use crate::models;

/// A safe, index-ready document derived from a provider record: only known
/// fields are projected into the title, body, and search attributes.
pub struct SafeDocument {
    pub title: String,
    pub body: String,
    /// (attribute_key, value) pairs published on the document for the
    /// operator registry (`location:`, `position:`, ...). Keys must match the
    /// `search_operators` advertised in the connector manifest.
    pub attributes: Vec<(String, String)>,
}

/// Safely format an org master item's title, content, and attributes from
/// known fields only. `attr_key` is the search attribute key for the entity
/// (e.g. `department`, `office_location`); a `{attr_key}_code` attribute is
/// added when the record carries a code.
pub fn format_org_master_item(item: &JsonValue, attr_key: &'static str) -> SafeDocument {
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

    let mut attributes = vec![(attr_key.to_string(), title.to_string())];
    if !code.is_empty() {
        attributes.push((format!("{attr_key}_code"), code.to_string()));
    }

    SafeDocument {
        title: title.to_string(),
        body: safe_body,
        attributes,
    }
}

/// Safely format a department master record. The departmentlist endpoint uses
/// `department_name`/`department_code` rather than the generic org-master
/// `name`/`code` convention, so departments get a dedicated mapper.
pub fn format_department_item(item: &JsonValue) -> SafeDocument {
    let title = item
        .get("department_name")
        .and_then(JsonValue::as_str)
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            item.get("name")
                .and_then(JsonValue::as_str)
                .filter(|s| !s.trim().is_empty())
        })
        .unwrap_or("Department")
        .to_string();
    let code = item
        .get("department_code")
        .and_then(JsonValue::as_str)
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            item.get("code")
                .and_then(JsonValue::as_str)
                .filter(|s| !s.trim().is_empty())
        })
        .unwrap_or_default();

    let mut body = format!("# {title}\n\n- Code: {code}");
    for (label, key) in [
        ("Parent Department Code", "parent_department_code"),
        ("Top Department", "top_department"),
        ("Business Unit", "business_unit"),
        ("Cost Center", "cost_center"),
        ("Department HOD", "departments_hod"),
        ("Status", "status"),
        ("Effective From", "effective_from_date"),
    ] {
        if let Some(value) = item
            .get(key)
            .and_then(JsonValue::as_str)
            .filter(|s| !s.trim().is_empty())
        {
            body.push_str(&format!("\n- {label}: {value}"));
        }
    }

    let mut attributes = vec![("department".to_string(), title.clone())];
    if !code.is_empty() {
        attributes.push(("department_code".to_string(), code.to_string()));
    }
    SafeDocument {
        title,
        body,
        attributes,
    }
}

/// Safely format an office location master record. The officelocationlist
/// endpoint returns `work_area`/`loc_type_id`/`city`-style fields rather than
/// the generic org-master `name`/`code` convention, so office locations get a
/// dedicated mapper.
pub fn format_office_location_item(item: &JsonValue) -> SafeDocument {
    let work_area = item
        .get("work_area")
        .and_then(JsonValue::as_str)
        .filter(|s| !s.trim().is_empty());
    let city = item
        .get("city")
        .and_then(JsonValue::as_str)
        .filter(|s| !s.trim().is_empty());
    let title = match (work_area, city) {
        (Some(work_area), Some(city)) => format!("{work_area} — {city}"),
        (Some(work_area), None) => work_area.to_string(),
        (None, Some(city)) => city.to_string(),
        (None, None) => item
            .get("company_name")
            .and_then(JsonValue::as_str)
            .filter(|s| !s.trim().is_empty())
            .unwrap_or("Office Location")
            .to_string(),
    };
    let code = item
        .get("loc_type_id")
        .and_then(JsonValue::as_str)
        .unwrap_or_default();

    let mut body = format!("# {title}");
    for (label, key) in [
        ("Work Area", "work_area"),
        ("Company", "company_name"),
        ("Address", "address"),
        ("City", "city"),
        ("State", "state"),
        ("Country", "country"),
        ("Pin Code", "pin_code"),
        ("Location Head", "location_head"),
        ("Status", "status"),
    ] {
        if let Some(value) = item
            .get(key)
            .and_then(JsonValue::as_str)
            .filter(|s| !s.trim().is_empty())
        {
            body.push_str(&format!("\n- {label}: {value}"));
        }
    }

    let mut attributes = vec![("office_location".to_string(), title.clone())];
    if !code.is_empty() {
        attributes.push(("office_location_code".to_string(), code.to_string()));
    }
    SafeDocument {
        title,
        body,
        attributes,
    }
}

/// Read a string field from a provider item, preferring `key` and falling
/// back to `alias` so both Darwinbox response key variants are accepted.
pub fn field_with_alias<'a>(item: &'a JsonValue, key: &'a str, alias: &'a str) -> Option<&'a str> {
    item.get(key)
        .and_then(JsonValue::as_str)
        .or_else(|| item.get(alias).and_then(JsonValue::as_str))
}

/// Format a holiday document from the typed holiday-list item.
pub fn format_holiday_item(item: &models::HolidayItem) -> SafeDocument {
    let mut lines = vec![format!("# {}", item.name), format!("- Date: {}", item.date)];
    for (label, value) in [
        ("Repeats", item.holiday_repeats.as_deref()),
        ("Optional", item.is_optional.as_deref()),
        ("National", item.is_national.as_deref()),
    ] {
        if let Some(value) = value {
            lines.push(format!("- {label}: {value}"));
        }
    }

    let mut attributes = vec![("holiday_name".to_string(), item.name.clone())];
    attributes.push(("holiday_date".to_string(), item.date.clone()));

    SafeDocument {
        title: item.name.clone(),
        body: lines.join("\n"),
        attributes,
    }
}

/// Safely format a position item from known fields.
pub fn format_position_item(item: &JsonValue) -> SafeDocument {
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

    let mut attributes = vec![("position".to_string(), title.to_string())];
    if !code.is_empty() {
        attributes.push(("position_code".to_string(), code.to_string()));
    }

    SafeDocument {
        title: title.to_string(),
        body: safe_body,
        attributes,
    }
}

/// Safely format an ATS job from known fields.
/// Safely format a job-level master record. Darwinbox's joblevellist records
/// use `job_level`/`job_level_code`/`grade`/`status` rather than the generic
/// org-master `name`/`code` convention, so they get a dedicated mapper.
pub fn format_job_level_item(item: &JsonValue) -> SafeDocument {
    let title = item
        .get("job_level")
        .and_then(JsonValue::as_str)
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            item.get("grade")
                .and_then(JsonValue::as_str)
                .filter(|s| !s.trim().is_empty())
        })
        .unwrap_or("Job Level")
        .to_string();

    let mut body = format!("# {title}");
    for (label, key) in [
        ("Code", "job_level_code"),
        ("Grade", "grade"),
        ("Grade Code", "grade_code"),
        ("Status", "status"),
        ("Effective From", "effective_from"),
        ("Created Date", "created_date"),
        ("Updated Date", "updated_date"),
    ] {
        if let Some(value) = item
            .get(key)
            .and_then(JsonValue::as_str)
            .filter(|s| !s.trim().is_empty())
        {
            body.push_str(&format!("\n- {label}: {value}"));
        }
    }

    let mut attributes = vec![("job_level".to_string(), title.clone())];
    if let Some(code) = item
        .get("job_level_code")
        .and_then(JsonValue::as_str)
        .filter(|s| !s.trim().is_empty())
    {
        attributes.push(("job_level_code".to_string(), code.to_string()));
    }
    SafeDocument {
        title,
        body,
        attributes,
    }
}

/// Pull a column value from a columnar org-master row. Only string values are
/// projected; empty strings are treated as absent.
pub fn table_column<'a>(cols: &'a [String], row: &'a [JsonValue], label: &str) -> Option<&'a str> {
    let index = cols.iter().position(|col| col == label)?;
    match row.get(index) {
        Some(JsonValue::String(value)) if !value.trim().is_empty() => Some(value.as_str()),
        _ => None,
    }
}

/// Safely format a row from a columnar org-master response (`{cols, data}`,
/// e.g. employeeJobLevel/employeeLocation/employeeManager) into a document.
/// Only the provider's own column labels from a per-entity allowlist are
/// rendered, so no unknown field can leak into indexed content.
pub fn format_org_master_table_item(
    cols: &[String],
    row: &[JsonValue],
    content_type: &str,
) -> SafeDocument {
    const FALLBACK_COLS: &[&str] = &["Employee ID", "Name", "From", "To", "Event", "Sub Event"];
    let allowed: &[&str] = match content_type {
        "employee_job_level" => &[
            "Employee ID",
            "Name",
            "Job Level Name",
            "From",
            "To",
            "Event",
            "Sub Event",
        ],
        "employee_location" => &[
            "Employee ID",
            "Name",
            "Company Name",
            "Area",
            "City",
            "State",
            "Country",
            "Work Area Code",
            "Pin Code",
            "Location Head",
            "From",
            "To",
            "Event",
            "Sub Event",
        ],
        "employee_manager" => &[
            "Employee ID",
            "Name",
            "Manager Name",
            "From",
            "To",
            "Event",
            "Sub Event",
        ],
        _ => FALLBACK_COLS,
    };
    let attribute_column = match content_type {
        "employee_job_level" => "Job Level Name",
        "employee_location" => "Area",
        "employee_manager" => "Manager Name",
        _ => "",
    };

    let title = table_column(cols, row, "Name")
        .or_else(|| table_column(cols, row, "Employee ID"))
        .unwrap_or("Employee record")
        .to_string();
    let mut body = format!("# {title}");
    for column in allowed {
        if let Some(value) = table_column(cols, row, column) {
            body.push_str(&format!("\n- {column}: {value}"));
        }
    }

    let attribute_value = table_column(cols, row, attribute_column).unwrap_or(&title);
    let mut attributes = vec![(content_type.to_string(), attribute_value.to_string())];
    if let Some(employee_id) = table_column(cols, row, "Employee ID") {
        attributes.push(("employee_id".to_string(), employee_id.to_string()));
    }
    SafeDocument {
        title,
        body,
        attributes,
    }
}

pub fn format_ats_job_item(item: &JsonValue) -> SafeDocument {
    let title = item
        .get("job_title")
        .and_then(JsonValue::as_str)
        .unwrap_or("Job");
    let job_id = item.get("job_id").and_then(JsonValue::as_str).unwrap_or("");

    let safe_body = format!("# {title}\n\n- Job ID: {job_id}");

    SafeDocument {
        title: title.to_string(),
        body: safe_body,
        attributes: vec![
            ("job_title".to_string(), title.to_string()),
            ("job_id".to_string(), job_id.to_string()),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn department_mapper_formats_real_api_shape() {
        // Real departmentlist record: department_name/department_code, not the
        // generic name/code convention.
        let document = format_department_item(&serde_json::json!({
            "parent_company_id": "Example Corp",
            "department_name": "ARCHIVED--New Member Development",
            "department_code": "WWS_NMD",
            "parent_department_code": "SALES",
            "top_department": "Sales",
            "cost_center": "",
            "departments_hod": "Santosh Martin (WW1600)",
            "status": "Active",
            "effective_from_date": "05-07-2023"
        }));
        assert_eq!(document.title, "ARCHIVED--New Member Development");
        assert!(document.body.contains("- Code: WWS_NMD"));
        assert!(document.body.contains("- Parent Department Code: SALES"));
        assert!(document.body.contains("- Top Department: Sales"));
        assert!(
            document
                .body
                .contains("- Department HOD: Santosh Martin (WW1600)")
        );
        assert!(document.body.contains("- Status: Active"));
        assert!(document.body.contains("- Effective From: 05-07-2023"));
        assert!(document.attributes.contains(&(
            "department".to_string(),
            "ARCHIVED--New Member Development".to_string()
        )));
        assert!(
            document
                .attributes
                .contains(&("department_code".to_string(), "WWS_NMD".to_string()))
        );
        assert!(!document.title.contains("unknown"));
    }

    #[test]
    fn department_mapper_falls_back_to_generic_keys() {
        let document = format_department_item(&serde_json::json!({
            "name": "Corporate",
            "code": "CORP",
            "status": "active"
        }));
        assert_eq!(document.title, "Corporate");
        assert!(document.body.contains("- Code: CORP"));
    }

    #[test]
    fn office_location_mapper_formats_real_api_shape() {
        // Real officelocationlist record: work_area/loc_type_id/city fields,
        // none of which matched the generic org-master convention.
        let document = format_office_location_item(&serde_json::json!({
            "company_name": "Temporary Employees",
            "address": "Prestige Central Ground Floor, 36, Infantry Rd, Bengaluru, Karnataka 560001",
            "pin_code": " 560001",
            "city": "Bengaluru",
            "state": "Karnataka",
            "country": "India",
            "location_head": "",
            "work_area": "BLR01",
            "status": "Active",
            "loc_type_id": "5db93215ece4c"
        }));
        assert_eq!(document.title, "BLR01 — Bengaluru");
        assert!(document.body.contains("- Work Area: BLR01"));
        assert!(document.body.contains("- Company: Temporary Employees"));
        assert!(document.body.contains("- City: Bengaluru"));
        assert!(document.body.contains("- State: Karnataka"));
        assert!(document.body.contains("- Country: India"));
        assert!(document.body.contains("- Status: Active"));
        assert!(document.attributes.contains(&(
            "office_location".to_string(),
            "BLR01 — Bengaluru".to_string()
        )));
        assert!(document.attributes.contains(&(
            "office_location_code".to_string(),
            "5db93215ece4c".to_string()
        )));
        assert!(!document.title.contains("unknown"));
    }

    #[test]
    fn office_location_mapper_falls_back_to_company_name() {
        let document = format_office_location_item(&serde_json::json!({
            "company_name": "Example Corp",
            "loc_type_id": "abc123"
        }));
        assert_eq!(document.title, "Example Corp");
        assert!(
            document
                .attributes
                .contains(&("office_location_code".to_string(), "abc123".to_string()))
        );
    }

    #[test]
    fn holiday_mapper_formats_typed_item() {
        let item = models::HolidayItem {
            id: Some("a68f996eb7bec5".into()),
            name: "Independence Day".into(),
            date: "2026-08-15".into(),
            year: Some("2026".into()),
            holiday_repeats: Some("No".into()),
            is_optional: Some("No".into()),
            is_national: Some("Yes".into()),
        };
        let document = format_holiday_item(&item);
        assert_eq!(document.title, "Independence Day");
        assert!(document.body.contains("2026-08-15"));
        assert!(document.body.contains("- National: Yes"));
        assert!(
            document
                .attributes
                .contains(&("holiday_name".to_string(), "Independence Day".to_string()))
        );
        assert!(
            document
                .attributes
                .contains(&("holiday_date".to_string(), "2026-08-15".to_string()))
        );
    }

    #[test]
    fn holiday_mapper_tolerates_absent_optional_fields() {
        let item = models::HolidayItem {
            id: None,
            name: "Republic Day".into(),
            date: "2026-01-26".into(),
            year: None,
            holiday_repeats: None,
            is_optional: None,
            is_national: None,
        };
        let document = format_holiday_item(&item);
        assert_eq!(document.title, "Republic Day");
        assert_eq!(document.body, "# Republic Day\n- Date: 2026-01-26");
        assert!(
            document
                .attributes
                .contains(&("holiday_date".to_string(), "2026-01-26".to_string()))
        );
    }
}
