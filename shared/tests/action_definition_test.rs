use serde_json::json;
use shared::models::ActionDefinition;

fn action_json() -> serde_json::Value {
    json!({
        "name": "example",
        "description": "Example action",
        "input_schema": {},
        "mode": "read"
    })
}

#[test]
fn omitted_required_scopes_deserializes_as_undeclared() {
    let action: ActionDefinition = serde_json::from_value(action_json()).unwrap();

    assert_eq!(action.required_scopes, None);
}

#[test]
fn explicit_empty_required_scopes_remains_distinct_from_undeclared() {
    let mut value = action_json();
    value["required_scopes"] = json!([]);

    let action: ActionDefinition = serde_json::from_value(value).unwrap();

    assert_eq!(action.required_scopes, Some(Vec::new()));
}
