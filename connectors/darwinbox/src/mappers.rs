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
