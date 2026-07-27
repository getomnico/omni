use omni_connector_sdk::{AuthType, ServiceCredential, ServiceProvider, Source, SourceType};
use omni_darwinbox_connector::actions::{action_definitions, action_policies, execute_action};
use omni_darwinbox_connector::client::DarwinboxClient;
use omni_darwinbox_connector::config::{
    document_permissions, normalize_email, normalize_emails, DarwinboxSourceConfig, EmployeeField,
    EmployeeScope,
};
use omni_darwinbox_connector::credentials::DarwinboxCredentials;
use omni_darwinbox_connector::models::EmployeeRecord;
use serde_json::json;
use time::OffsetDateTime;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Build a test source for a given source config.
fn test_source(config: serde_json::Value) -> Source {
    serde_json::from_value(json!({
        "id": "source-1",
        "name": "Test Darwinbox",
        "source_type": "darwinbox",
        "config": config,
        "is_active": true,
        "is_deleted": false,
        "scope": "org",
        "user_filter_mode": "all",
        "created_by": "admin",
        "created_at": "2025-01-01T00:00:00Z",
        "updated_at": "2025-01-01T00:00:00Z",
    }))
    .expect("test_source should construct a valid Source")
}

fn test_actor(email: &str) -> Option<String> {
    Some(email.to_string())
}

fn test_credential() -> ServiceCredential {
    let now = OffsetDateTime::now_utc();
    ServiceCredential {
        id: "cred-1".to_string(),
        source_id: "source-1".to_string(),
        user_id: None,
        provider: ServiceProvider::Darwinbox,
        auth_type: AuthType::BasicAuth,
        principal_email: None,
        credentials: json!({
            "auth_type": "basic",
            "username": "api-user",
            "password": "secret",
            "api_key": "api-key",
            "dataset_key": "dataset-key"
        }),
        config: json!({}),
        expires_at: None,
        last_validated_at: None,
        created_at: now,
        updated_at: now,
    }
}

fn test_config(base_url: &str) -> serde_json::Value {
    json!({
        "base_url": base_url,
        "read_only": true,
        "employee_scope": { "mode": "include", "employee_ids": ["EMP001"] },
        "employee_fields": ["name", "employee_id", "company_email", "department"],
        "sync_modules": {},
        "action_modules": { "employee_self_service": true },
        "authorization": {
            "actions_enabled": true,
            "participant_emails": ["a@example.com"],
            "allowed_actions": ["get_my_leave_balance"],
            "max_batch_size": 1
        }
    })
}

#[tokio::test]
async fn client_fetch_employees_ignores_sensitive_unknown_fields() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/masterapi/employee"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "status": 1,
            "message": "ok",
            "employee_data": [{
                "employee_id": "EMP001",
                "first_name": "Asha",
                "company_email_id": "asha@example.com",
                "salary": "SECRET-SALARY",
                "bank_account": "SECRET-BANK"
            }]
        })))
        .mount(&server)
        .await;
    let config: DarwinboxSourceConfig = serde_json::from_value(test_config(&server.uri())).unwrap();
    let credentials = DarwinboxCredentials::Basic {
        username: "api-user".to_string(),
        password: "secret".to_string(),
        api_key: "api-key".to_string(),
        dataset_key: "dataset-key".to_string(),
    };
    let response = DarwinboxClient::new(&config, credentials)
        .unwrap()
        .fetch_employees(None, None)
        .await
        .unwrap();
    assert_eq!(
        response.employee_data[0].employee_id.as_deref(),
        Some("EMP001")
    );
}

#[tokio::test]
async fn duplicate_self_identity_is_rejected() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/masterapi/employee"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "employee_data": [
                {"employee_id": "EMP001", "company_email_id": "a@example.com"},
                {"employee_id": "EMP002", "company_email_id": "A@example.com"}
            ]
        })))
        .mount(&server)
        .await;

    let mut config = test_config(&server.uri());
    config["authorization"]["allowed_actions"] = json!(["get_my_profile"]);
    let error = execute_action(
        "get_my_profile",
        json!({}),
        Some(test_credential()),
        Some(test_source(config.clone())),
        test_actor("a@example.com"),
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("multiple Darwinbox employees"));
}

#[tokio::test]
async fn token_errors_do_not_expose_provider_body() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/oauth/v2token"))
        .respond_with(ResponseTemplate::new(401).set_body_string("SECRET-TOKEN-DIAGNOSTIC"))
        .mount(&server)
        .await;
    let config: DarwinboxSourceConfig = serde_json::from_value(test_config(&server.uri())).unwrap();
    let credentials = DarwinboxCredentials::ClientCredentials {
        client_id: "client".to_string(),
        client_secret: "secret".to_string(),
        api_key: None,
        dataset_key: "dataset".to_string(),
    };
    let error = DarwinboxClient::new(&config, credentials)
        .unwrap()
        .fetch_employees(None, None)
        .await
        .unwrap_err();
    assert!(!error.to_string().contains("SECRET-TOKEN-DIAGNOSTIC"));
    assert!(error.to_string().contains("HTTP 401"));
}

#[tokio::test]
async fn action_requires_trusted_source_config() {
    let error = execute_action(
        "get_my_leave_balance",
        json!({ "base_url": "https://attacker.example", "read_only": false }),
        Some(test_credential()),
        None, // no source — should fail with "requires a trusted source"
        None, // no actor
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("trusted source"));
}

#[tokio::test]
async fn self_action_rejects_spoofed_identity_before_provider_call() {
    let config = test_config("https://example.darwinbox.in");
    let error = execute_action(
        "get_my_leave_balance",
        json!({
            "employee_no": "EMP-B"
        }),
        Some(test_credential()),
        Some(test_source(config)),
        test_actor("a@example.com"),
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("employee_no"));
}

#[test]
fn config_defaults_fail_closed() {
    let config: DarwinboxSourceConfig = serde_json::from_value(json!({
        "base_url": "https://example.darwinbox.in"
    }))
    .unwrap();
    assert!(config.read_only);
    assert!(!config.sync_modules.employee_directory);
    assert!(!config.sync_modules.deleted_employees);
    assert!(!config.action_modules.employee_self_service);
    assert!(!config.authorization.actions_enabled);
}

#[test]
fn employee_directory_requires_explicit_scope_and_fields() {
    let config: DarwinboxSourceConfig = serde_json::from_value(json!({
        "base_url": "https://example.darwinbox.in",
        "sync_modules": { "employee_directory": true }
    }))
    .unwrap();
    let errors = config.validate().unwrap_err().join("; ");
    assert!(errors.contains("employee_scope"));
    assert!(errors.contains("employee_fields"));
}

#[test]
fn deprecated_permission_switch_is_rejected() {
    let config: DarwinboxSourceConfig = serde_json::from_value(json!({
        "base_url": "https://example.darwinbox.in",
        "authorization": { "use_darwinbox_permissions": true }
    }))
    .unwrap();
    assert!(config
        .validate()
        .unwrap_err()
        .iter()
        .any(|e| e.contains("unsupported")));
}

#[test]
fn high_risk_raw_action_families_are_rejected() {
    let config: DarwinboxSourceConfig = serde_json::from_value(json!({
        "base_url": "https://example.darwinbox.in",
        "authorization": {
            "actions_enabled": true,
            "participant_emails": ["admin@example.com"],
            "allowed_actions": ["add_pending_employee"]
        },
        "action_modules": { "hr_operations": true }
    }))
    .unwrap();
    assert!(config
        .validate()
        .unwrap_err()
        .iter()
        .any(|e| e.contains("not implemented")));
}

#[test]
fn employee_scope_wire_shape_matches_ui() {
    let config: DarwinboxSourceConfig = serde_json::from_value(json!({
        "base_url": "https://example.darwinbox.in",
        "employee_scope": { "mode": "include", "departments": ["Engineering"] }
    }))
    .unwrap();
    assert!(matches!(
        config.employee_scope,
        Some(EmployeeScope::Include { .. })
    ));
}

#[test]
fn action_manifest_and_policy_modes_match() {
    let definitions = action_definitions();
    let policies = action_policies();
    assert_eq!(definitions.len(), policies.len());
    for definition in definitions {
        let matches: Vec<_> = policies
            .iter()
            .filter(|p| p.name == definition.name)
            .collect();
        assert_eq!(matches.len(), 1, "policy drift for {}", definition.name);
        assert_eq!(matches[0].mode, definition.mode);
        assert_eq!(
            matches[0].is_write,
            definition.mode == omni_connector_sdk::ActionMode::Write
        );
    }
}

#[test]
fn document_acl_is_non_public_and_uses_named_participants() {
    let mut value = test_config("https://example.darwinbox.in");
    value["authorization"]["hr_admin_emails"] = json!(["hr@example.com"]);
    let config: DarwinboxSourceConfig = serde_json::from_value(value).unwrap();
    let permissions = document_permissions(
        "employee_profile",
        &config,
        "source-1",
        Some("employee@example.com"),
    );
    assert!(!permissions.public);
    assert!(permissions.groups.is_empty());
    assert!(permissions.users.contains(&"a@example.com".to_string()));
    assert!(permissions
        .users
        .contains(&"employee@example.com".to_string()));
    assert!(permissions.users.contains(&"hr@example.com".to_string()));
}

#[test]
fn unknown_document_type_has_empty_acl() {
    let config: DarwinboxSourceConfig =
        serde_json::from_value(test_config("https://example.darwinbox.in")).unwrap();
    let permissions = document_permissions("future_sensitive_type", &config, "source", None);
    assert!(!permissions.public);
    assert!(permissions.users.is_empty());
    assert!(permissions.groups.is_empty());
}

#[test]
fn selected_missing_field_never_widens_employee_content() {
    let employee = EmployeeRecord {
        employee_id: Some("EMP001".to_string()),
        first_name: Some("Sensitive Name".to_string()),
        department_name: Some("Secret Department".to_string()),
        ..Default::default()
    };
    let content = employee.content_filtered(&[EmployeeField::CompanyEmail]);
    assert!(content.is_empty());
    assert!(!content.contains("EMP001"));
    assert!(!content.contains("Sensitive"));
    assert!(!content.contains("Secret"));
}

#[test]
fn untyped_sync_modules_are_rejected() {
    let config: DarwinboxSourceConfig = serde_json::from_value(json!({
        "base_url": "https://example.darwinbox.in",
        "sync_modules": { "holidays": true }
    }))
    .unwrap();
    assert!(config
        .validate()
        .unwrap_err()
        .iter()
        .any(|error| error.contains("unavailable")));
}

#[test]
fn url_and_email_normalization_are_fail_closed() {
    let config: DarwinboxSourceConfig = serde_json::from_value(json!({
        "base_url": "http://localhost.attacker.example"
    }))
    .unwrap();
    assert!(config
        .validate()
        .unwrap_err()
        .iter()
        .any(|e| e.contains("HTTPS")));
    assert_eq!(normalize_email(" A@Example.COM "), "a@example.com");
    assert_eq!(
        normalize_emails(&["A@B.com".into(), "a@b.com".into()]).len(),
        1
    );
}
