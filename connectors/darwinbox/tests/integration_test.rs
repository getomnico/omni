use omni_connector_sdk::{AuthType, ServiceCredential, ServiceProvider, Source};
use omni_darwinbox_connector::actions::{action_definitions, action_policies, execute_action};
use omni_darwinbox_connector::client::DarwinboxClient;
use omni_darwinbox_connector::config::{
    APPROVED_EMPLOYEE_FIELDS, DarwinboxSourceConfig, EmployeeField, document_permissions,
    normalize_email, normalize_emails,
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
        "authorization": {
            "participant_emails": ["a@example.com"],
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

    let config = test_config(&server.uri());
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
async fn client_credentials_business_request_uses_bearer_token_header() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/oauth/v2token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "client-cred-token",
            "expires_in": 3600,
            "token_type": "Bearer"
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/masterapi/employee"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "status": 1,
            "message": "ok",
            "employee_data": [{
                "employee_id": "EMP001",
                "first_name": "Asha",
                "company_email_id": "asha@example.com"
            }]
        })))
        .mount(&server)
        .await;

    let config: DarwinboxSourceConfig = serde_json::from_value(test_config(&server.uri())).unwrap();
    let credentials = DarwinboxCredentials::ClientCredentials {
        client_id: "client".to_string(),
        client_secret: "secret".to_string(),
        api_key: None,
        dataset_key: "dataset".to_string(),
    };
    DarwinboxClient::new(&config, credentials)
        .unwrap()
        .fetch_employees(None, None)
        .await
        .unwrap();

    let business_requests = server
        .received_requests()
        .await
        .expect("mock server should record requests")
        .into_iter()
        .filter(|request| request.url.path() == "/masterapi/employee")
        .collect::<Vec<_>>();
    assert_eq!(business_requests.len(), 1);
    let authorization = business_requests[0]
        .headers
        .get("authorization")
        .expect("business request must carry Authorization")
        .to_str()
        .unwrap();
    assert_eq!(authorization, "Bearer client-cred-token");
    assert!(
        business_requests[0].headers.get("TOKEN").is_none(),
        "business request must not use the legacy TOKEN header"
    );
}

#[tokio::test]
async fn dynamic_token_business_request_uses_bearer_token_header() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/oauth/v1token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "dynamic-token",
            "expires_in": 3600,
            "token_type": "Bearer"
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/masterapi/employee"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "status": 1,
            "message": "ok",
            "employee_data": []
        })))
        .mount(&server)
        .await;

    let config: DarwinboxSourceConfig = serde_json::from_value(test_config(&server.uri())).unwrap();
    let credentials = DarwinboxCredentials::DynamicToken {
        client_id: "client".to_string(),
        client_secret: "secret".to_string(),
        grant_type: "authorization_code".to_string(),
        code: Some("auth-code".to_string()),
        refresh_token: None,
        api_key: None,
        dataset_key: "dataset".to_string(),
    };
    DarwinboxClient::new(&config, credentials)
        .unwrap()
        .fetch_employees(None, None)
        .await
        .unwrap();

    let business_requests = server
        .received_requests()
        .await
        .expect("mock server should record requests")
        .into_iter()
        .filter(|request| request.url.path() == "/masterapi/employee")
        .collect::<Vec<_>>();
    assert_eq!(business_requests.len(), 1);
    let authorization = business_requests[0]
        .headers
        .get("authorization")
        .expect("business request must carry Authorization")
        .to_str()
        .unwrap();
    assert_eq!(authorization, "Bearer dynamic-token");
    assert!(
        business_requests[0].headers.get("TOKEN").is_none(),
        "business request must not use the legacy TOKEN header"
    );
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
fn config_defaults_are_read_only_and_open_participation() {
    let config: DarwinboxSourceConfig = serde_json::from_value(json!({
        "base_url": "https://example.darwinbox.in"
    }))
    .unwrap();
    assert!(config.read_only);
    assert_eq!(config.participant_mode(), "all");
    assert!(config.validate().is_ok());
}

#[test]
fn approved_employee_fields_always_include_company_email() {
    assert!(APPROVED_EMPLOYEE_FIELDS.contains(&EmployeeField::CompanyEmail));
    assert!(APPROVED_EMPLOYEE_FIELDS.contains(&EmployeeField::Name));
    assert!(!APPROVED_EMPLOYEE_FIELDS.is_empty());
}

#[test]
fn participant_mode_defaults_to_everyone_and_derives_legacy_allowlist() {
    // Explicit "all" mode needs no emails and admits any caller.
    let everyone: DarwinboxSourceConfig = serde_json::from_value(json!({
        "base_url": "https://example.darwinbox.in",
        "authorization": {
            "participant_mode": "all"
        }
    }))
    .unwrap();
    assert!(everyone.validate().is_ok());
    assert!(everyone.is_action_participant("anyone@example.com"));
    assert!(everyone.is_action_participant("another@example.com"));

    // Legacy configs carry only emails: a non-empty allowlist stays restricted.
    let legacy: DarwinboxSourceConfig = serde_json::from_value(json!({
        "base_url": "https://example.darwinbox.in",
        "authorization": {
            "participant_emails": ["a@example.com"]
        }
    }))
    .unwrap();
    assert_eq!(legacy.participant_mode(), "allowlist");
    assert!(legacy.is_action_participant("a@example.com"));
    assert!(!legacy.is_action_participant("outsider@example.com"));
    assert!(legacy.validate().is_ok());

    // Explicit allowlist with no emails is invalid; unknown mode is invalid.
    let empty_allowlist: DarwinboxSourceConfig = serde_json::from_value(json!({
        "base_url": "https://example.darwinbox.in",
        "authorization": {
            "participant_mode": "allowlist"
        }
    }))
    .unwrap();
    assert!(
        empty_allowlist
            .validate()
            .unwrap_err()
            .iter()
            .any(|e| e.contains("participant_emails"))
    );
    let bogus: DarwinboxSourceConfig = serde_json::from_value(json!({
        "base_url": "https://example.darwinbox.in",
        "authorization": {
            "participant_mode": "friends",
            "participant_emails": ["a@example.com"]
        }
    }))
    .unwrap();
    assert!(
        bogus
            .validate()
            .unwrap_err()
            .iter()
            .any(|e| e.contains("participant_mode"))
    );
}

#[test]
fn action_manifest_and_policy_modes_match() {
    let definitions = action_definitions();
    let policies: Vec<_> = action_policies()
        .iter()
        .filter(|policy| policy.available)
        .collect();
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
fn employee_profile_acl_is_user_private() {
    let config: DarwinboxSourceConfig =
        serde_json::from_value(test_config("https://example.darwinbox.in")).unwrap();
    let permissions = document_permissions(
        "employee_profile",
        &config,
        "source-1",
        Some("employee@example.com"),
    );
    assert!(!permissions.public);
    assert!(
        permissions
            .users
            .contains(&"employee@example.com".to_string())
    );
    assert!(!permissions.users.contains(&"a@example.com".to_string()));
}

#[test]
fn org_master_documents_are_public() {
    let config: DarwinboxSourceConfig =
        serde_json::from_value(test_config("https://example.darwinbox.in")).unwrap();
    for content_type in &[
        "department",
        "designation",
        "office_location",
        "business_unit",
        "division",
        "cost_center",
        "group_company",
        "holiday",
        "position",
        "job",
        "ats_job",
    ] {
        let permissions = document_permissions(content_type, &config, "source-1", None);
        assert!(permissions.public, "{content_type} should be public");
        assert!(
            permissions.users.is_empty(),
            "{content_type} should have no user restrictions"
        );
    }
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
fn url_and_email_normalization_are_fail_closed() {
    let config: DarwinboxSourceConfig = serde_json::from_value(json!({
        "base_url": "http://localhost.attacker.example"
    }))
    .unwrap();
    assert!(
        config
            .validate()
            .unwrap_err()
            .iter()
            .any(|e| e.contains("HTTPS"))
    );
    assert_eq!(normalize_email(" A@Example.COM "), "a@example.com");
    assert_eq!(
        normalize_emails(&["A@B.com".into(), "a@b.com".into()]).len(),
        1
    );
}

#[test]
fn org_master_mappers_emit_searchable_attributes() {
    use omni_darwinbox_connector::mappers;
    let item = serde_json::json!({
        "name": "Corporate",
        "code": "CORP",
        "description": "Group division",
        "status": "active"
    });
    let document = mappers::format_org_master_item(&item, "division");
    assert_eq!(document.title, "Corporate");
    assert!(document.body.contains("CORP"));
    assert!(
        document
            .attributes
            .contains(&("division".to_string(), "Corporate".to_string()))
    );
    assert!(
        document
            .attributes
            .contains(&("division_code".to_string(), "CORP".to_string()))
    );

    let holiday = mappers::format_holiday_item(&serde_json::json!({
        "holiday_name": "Republic Day",
        "holiday_date": "2025-01-26",
        "description": "National holiday"
    }));
    assert!(
        holiday
            .attributes
            .contains(&("holiday_date".to_string(), "2025-01-26".to_string()))
    );

    let position = mappers::format_position_item(&serde_json::json!({
        "name": "Engineering Manager",
        "position_code": "EM-1",
        "status": "open"
    }));
    assert!(
        position
            .attributes
            .contains(&("position".to_string(), "Engineering Manager".to_string()))
    );
}

#[tokio::test]
async fn action_denied_by_provider_returns_not_allowed_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/masterapi/employee"))
        .respond_with(ResponseTemplate::new(401).set_body_string("SECRET-DENIAL-BODY"))
        .mount(&server)
        .await;

    let config = test_config(&server.uri());
    let error = execute_action(
        "get_my_profile",
        json!({}),
        Some(test_credential()),
        Some(test_source(config)),
        test_actor("a@example.com"),
    )
    .await
    .unwrap_err();
    let message = error.to_string();
    assert!(
        message.contains("not allowed"),
        "expected not-allowed error, got: {message}"
    );
    assert!(
        !message.contains("SECRET-DENIAL-BODY"),
        "denial body must never be echoed"
    );
}

#[tokio::test]
async fn client_reports_denied_access_as_not_permitted() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/orgmasterapi/divisionlist"))
        .respond_with(ResponseTemplate::new(403).set_body_string("SECRET"))
        .mount(&server)
        .await;
    let config: DarwinboxSourceConfig = serde_json::from_value(test_config(&server.uri())).unwrap();
    let credentials = DarwinboxCredentials::Basic {
        username: "api-user".to_string(),
        password: "secret".to_string(),
        api_key: "api-key".to_string(),
        dataset_key: "dataset-key".to_string(),
    };
    let error = DarwinboxClient::new(&config, credentials)
        .unwrap()
        .fetch_org_master("/orgmasterapi/divisionlist")
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        omni_darwinbox_connector::client::DarwinboxApiError::NotPermitted { .. }
    ));
}
