//! Typed mappers from Darwinbox provider models to Omni document events.
//! Each entity type has its own mapper so raw provider fields are never
//! serialized into indexed content, metadata, attributes, or logs.

use serde_json::Value as JsonValue;

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

/// Safely format a holiday item from known fields.
pub fn format_holiday_item(item: &JsonValue) -> SafeDocument {
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

    let mut attributes = vec![("holiday_name".to_string(), name.to_string())];
    if !date.is_empty() {
        attributes.push(("holiday_date".to_string(), date.to_string()));
    }

    SafeDocument {
        title: name.to_string(),
        body: safe_body,
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
