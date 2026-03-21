use serde_json::Value as JsonValue;
use shared::models::{ConnectorEvent, SearchOperator};
use std::collections::HashMap;
use tracing::debug;

#[derive(Debug, Clone)]
pub struct ExtractedPerson {
    pub email: String,
    pub display_name: Option<String>,
}

/// Walk a JSON Schema object and return the keys of properties whose `format` is `"email"`.
/// Handles both direct properties and array items with format: email.
/// Returns tuples of (field_path, is_array).
fn find_email_fields(schema: &JsonValue) -> Vec<(String, bool)> {
    let mut result = Vec::new();

    let properties = match schema.get("properties").and_then(|p| p.as_object()) {
        Some(props) => props,
        None => return result,
    };

    for (key, prop_schema) in properties {
        // Direct string with format: email
        if prop_schema.get("type").and_then(|t| t.as_str()) == Some("string")
            && prop_schema.get("format").and_then(|f| f.as_str()) == Some("email")
        {
            result.push((key.clone(), false));
            continue;
        }

        // Array of strings with format: email
        if prop_schema.get("type").and_then(|t| t.as_str()) == Some("array") {
            if let Some(items) = prop_schema.get("items") {
                if items.get("type").and_then(|t| t.as_str()) == Some("string")
                    && items.get("format").and_then(|f| f.as_str()) == Some("email")
                {
                    result.push((key.clone(), true));
                }
            }
        }
    }

    result
}

/// Extract email values from a JSONB object using the email field paths from the schema.
fn extract_emails_from_json(data: &JsonValue, email_fields: &[(String, bool)]) -> Vec<String> {
    let mut emails = Vec::new();

    let obj = match data.as_object() {
        Some(o) => o,
        None => return emails,
    };

    for (key, is_array) in email_fields {
        if let Some(value) = obj.get(key) {
            if *is_array {
                if let Some(arr) = value.as_array() {
                    for item in arr {
                        if let Some(email) = item.as_str() {
                            if is_plausible_email(email) {
                                emails.push(email.to_lowercase());
                            }
                        }
                    }
                }
            } else if let Some(email) = value.as_str() {
                if is_plausible_email(email) {
                    emails.push(email.to_lowercase());
                }
            }
        }
    }

    emails
}

fn is_plausible_email(s: &str) -> bool {
    let s = s.trim();
    !s.is_empty() && s.contains('@') && s.contains('.')
}

/// Extract people from a connector event using the connector's declared schemas.
///
/// Extraction sources (in order):
/// 1. permissions.users — always emails
/// 2. metadata.author — may be an email
/// 3. Schema-driven: extra_schema fields with format: email → look in metadata.extra
/// 4. Schema-driven: attributes_schema fields with format: email → look in attributes
/// 5. Search operators with value_type: person → look in attributes
pub fn extract_people(
    extra_schema: Option<&JsonValue>,
    attributes_schema: Option<&JsonValue>,
    search_operators: &[SearchOperator],
    event: &ConnectorEvent,
) -> Vec<ExtractedPerson> {
    let mut seen: HashMap<String, ExtractedPerson> = HashMap::new();

    let (metadata, permissions, attributes) = match event {
        ConnectorEvent::DocumentCreated {
            metadata,
            permissions,
            attributes,
            ..
        } => (Some(metadata), Some(permissions), attributes.as_ref()),
        ConnectorEvent::DocumentUpdated {
            metadata,
            permissions,
            attributes,
            ..
        } => (Some(metadata), permissions.as_ref(), attributes.as_ref()),
        ConnectorEvent::DocumentDeleted { .. } | ConnectorEvent::GroupMembershipSync { .. } => {
            return vec![]
        }
    };

    // 1. permissions.users
    if let Some(perms) = permissions {
        for email in &perms.users {
            let lower = email.to_lowercase();
            if is_plausible_email(&lower) {
                seen.entry(lower.clone())
                    .or_insert_with(|| ExtractedPerson {
                        email: lower,
                        display_name: None,
                    });
            }
        }
    }

    // 2. metadata.author
    if let Some(meta) = metadata {
        if let Some(author) = &meta.author {
            let trimmed = author.trim();
            if is_plausible_email(trimmed) {
                let lower = trimmed.to_lowercase();
                seen.entry(lower.clone())
                    .or_insert_with(|| ExtractedPerson {
                        email: lower,
                        display_name: None,
                    });
            }
        }
    }

    // 3. Schema-driven: extra_schema → metadata.extra
    if let (Some(schema), Some(meta)) = (extra_schema, metadata) {
        let email_fields = find_email_fields(schema);
        if !email_fields.is_empty() {
            if let Some(extra) = &meta.extra {
                let extra_json =
                    serde_json::to_value(extra).unwrap_or(JsonValue::Object(Default::default()));
                for email in extract_emails_from_json(&extra_json, &email_fields) {
                    seen.entry(email.clone())
                        .or_insert_with(|| ExtractedPerson {
                            email,
                            display_name: None,
                        });
                }
            }
        }
    }

    // 4. Schema-driven: attributes_schema → attributes
    if let (Some(schema), Some(attrs)) = (attributes_schema, attributes) {
        let email_fields = find_email_fields(schema);
        if !email_fields.is_empty() {
            let attrs_json =
                serde_json::to_value(attrs).unwrap_or(JsonValue::Object(Default::default()));
            for email in extract_emails_from_json(&attrs_json, &email_fields) {
                seen.entry(email.clone())
                    .or_insert_with(|| ExtractedPerson {
                        email,
                        display_name: None,
                    });
            }
        }
    }

    // 5. Search operators with value_type: person → look in attributes
    if let Some(attrs) = attributes {
        for op in search_operators {
            if op.value_type == "person" {
                if let Some(value) = attrs.get(&op.attribute_key) {
                    if let Some(email) = value.as_str() {
                        if is_plausible_email(email) {
                            let lower = email.to_lowercase();
                            seen.entry(lower.clone())
                                .or_insert_with(|| ExtractedPerson {
                                    email: lower,
                                    display_name: None,
                                });
                        }
                    }
                }
            }
        }
    }

    let people: Vec<_> = seen.into_values().collect();
    if !people.is_empty() {
        debug!("Extracted {} people from event", people.len());
    }
    people
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use shared::models::{DocumentMetadata, DocumentPermissions};
    use std::collections::HashMap;

    #[test]
    fn test_find_email_fields_simple() {
        let schema = json!({
            "type": "object",
            "properties": {
                "sender": { "type": "string", "format": "email" },
                "thread_id": { "type": "string" },
                "recipients": {
                    "type": "array",
                    "items": { "type": "string", "format": "email" }
                },
                "labels": {
                    "type": "array",
                    "items": { "type": "string" }
                }
            }
        });

        let fields = find_email_fields(&schema);
        assert_eq!(fields.len(), 2);
        assert!(fields.contains(&("sender".to_string(), false)));
        assert!(fields.contains(&("recipients".to_string(), true)));
    }

    #[test]
    fn test_find_email_fields_empty_schema() {
        let schema = json!({});
        assert!(find_email_fields(&schema).is_empty());

        let schema = json!({ "type": "object" });
        assert!(find_email_fields(&schema).is_empty());
    }

    #[test]
    fn test_extract_emails_from_json() {
        let data = json!({
            "sender": "alice@example.com",
            "recipients": ["bob@example.com", "carol@example.com"],
            "thread_id": "abc123"
        });

        let fields = vec![
            ("sender".to_string(), false),
            ("recipients".to_string(), true),
        ];

        let emails = extract_emails_from_json(&data, &fields);
        assert_eq!(emails.len(), 3);
        assert!(emails.contains(&"alice@example.com".to_string()));
        assert!(emails.contains(&"bob@example.com".to_string()));
        assert!(emails.contains(&"carol@example.com".to_string()));
    }

    #[test]
    fn test_extract_people_from_permissions() {
        let event = ConnectorEvent::DocumentCreated {
            sync_run_id: "run-1".to_string(),
            source_id: "src-1".to_string(),
            document_id: "doc-1".to_string(),
            content_id: "content-1".to_string(),
            metadata: DocumentMetadata::default(),
            permissions: DocumentPermissions {
                public: false,
                users: vec![
                    "alice@example.com".to_string(),
                    "bob@example.com".to_string(),
                ],
                groups: vec![],
            },
            attributes: None,
        };

        let people = extract_people(None, None, &[], &event);
        assert_eq!(people.len(), 2);
        let emails: Vec<_> = people.iter().map(|p| p.email.as_str()).collect();
        assert!(emails.contains(&"alice@example.com"));
        assert!(emails.contains(&"bob@example.com"));
    }

    #[test]
    fn test_extract_people_schema_driven() {
        let extra_schema = json!({
            "type": "object",
            "properties": {
                "sender": { "type": "string", "format": "email" },
                "participants": {
                    "type": "array",
                    "items": { "type": "string", "format": "email" }
                }
            }
        });

        let mut extra = HashMap::new();
        extra.insert("sender".to_string(), json!("alice@example.com"));
        extra.insert(
            "participants".to_string(),
            json!(["bob@example.com", "carol@example.com"]),
        );

        let event = ConnectorEvent::DocumentCreated {
            sync_run_id: "run-1".to_string(),
            source_id: "src-1".to_string(),
            document_id: "doc-1".to_string(),
            content_id: "content-1".to_string(),
            metadata: DocumentMetadata {
                extra: Some(extra),
                ..Default::default()
            },
            permissions: DocumentPermissions {
                public: true,
                users: vec![],
                groups: vec![],
            },
            attributes: None,
        };

        let people = extract_people(Some(&extra_schema), None, &[], &event);
        assert_eq!(people.len(), 3);
    }

    #[test]
    fn test_extract_people_deduplicates() {
        let event = ConnectorEvent::DocumentCreated {
            sync_run_id: "run-1".to_string(),
            source_id: "src-1".to_string(),
            document_id: "doc-1".to_string(),
            content_id: "content-1".to_string(),
            metadata: DocumentMetadata {
                author: Some("alice@example.com".to_string()),
                ..Default::default()
            },
            permissions: DocumentPermissions {
                public: false,
                users: vec!["alice@example.com".to_string()],
                groups: vec![],
            },
            attributes: None,
        };

        let people = extract_people(None, None, &[], &event);
        assert_eq!(people.len(), 1);
        assert_eq!(people[0].email, "alice@example.com");
    }

    #[test]
    fn test_extract_people_from_search_operators() {
        let operators = vec![SearchOperator {
            operator: "assignee".to_string(),
            attribute_key: "assignee_email".to_string(),
            value_type: "person".to_string(),
        }];

        let mut attrs = HashMap::new();
        attrs.insert("assignee_email".to_string(), json!("dev@example.com"));

        let event = ConnectorEvent::DocumentCreated {
            sync_run_id: "run-1".to_string(),
            source_id: "src-1".to_string(),
            document_id: "doc-1".to_string(),
            content_id: "content-1".to_string(),
            metadata: DocumentMetadata::default(),
            permissions: DocumentPermissions {
                public: true,
                users: vec![],
                groups: vec![],
            },
            attributes: Some(attrs),
        };

        let people = extract_people(None, None, &operators, &event);
        assert_eq!(people.len(), 1);
        assert_eq!(people[0].email, "dev@example.com");
    }

    #[test]
    fn test_extract_people_skips_deletes() {
        let event = ConnectorEvent::DocumentDeleted {
            sync_run_id: "run-1".to_string(),
            source_id: "src-1".to_string(),
            document_id: "doc-1".to_string(),
        };

        let people = extract_people(None, None, &[], &event);
        assert!(people.is_empty());
    }
}
