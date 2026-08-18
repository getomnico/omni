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
        "job_level",
        "employee_job_level",
        "employee_location",
        "employee_manager",
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

    let holiday = mappers::format_holiday_item(&omni_darwinbox_connector::models::HolidayItem {
        id: Some("a68f996eb7bec5".into()),
        name: "Republic Day".into(),
        date: "2025-01-26".into(),
        year: Some("2025".into()),
        holiday_repeats: Some("No".into()),
        is_optional: Some("No".into()),
        is_national: Some("Yes".into()),
    });
    assert_eq!(holiday.title, "Republic Day");
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

#[test]
fn job_level_mapper_uses_provider_field_names() {
    use omni_darwinbox_connector::mappers;
    // Real joblevellist record: job_level/job_level_code/grade/status, not the
    // generic name/code convention.
    let document = mappers::format_job_level_item(&serde_json::json!({
        "job_level": "Senior Manager",
        "job_level_code": "006",
        "grade": "Senior Manager",
        "grade_code": "",
        "status": "Active",
        "created_date": "21-10-2019 20:26:44",
        "updated_date": "12-03-2025 11:39:44",
        "effective_from": "12-03-2025 00:00:00"
    }));
    assert_eq!(document.title, "Senior Manager");
    assert!(document.body.contains("Code: 006"));
    assert!(document.body.contains("Status: Active"));
    assert!(
        document
            .body
            .contains("Effective From: 12-03-2025 00:00:00")
    );
    assert!(
        document
            .attributes
            .contains(&("job_level".to_string(), "Senior Manager".to_string()))
    );
    assert!(
        document
            .attributes
            .contains(&("job_level_code".to_string(), "006".to_string()))
    );
}

#[test]
fn columnar_org_master_mapper_projects_provider_columns() {
    use omni_darwinbox_connector::mappers;
    // Real employeeJobLevel response shape: `cols` + `data` rows (arrays).
    let cols = vec![
        "Employee ID".to_string(),
        "Name".to_string(),
        "Job Level Name".to_string(),
        "From".to_string(),
        "To".to_string(),
        "Event".to_string(),
        "Sub Event".to_string(),
    ];
    let row = serde_json::json!([
        "WWITest2",
        "Dummy1 Test2",
        "Lead (003)",
        "05-08-2025",
        "13-04-2025",
        "",
        ""
    ]);
    let document =
        mappers::format_org_master_table_item(&cols, row.as_array().unwrap(), "employee_job_level");
    assert_eq!(document.title, "Dummy1 Test2");
    assert!(document.body.contains("Employee ID: WWITest2"));
    assert!(document.body.contains("Job Level Name: Lead (003)"));
    assert!(document.body.contains("From: 05-08-2025"));
    assert!(
        document
            .attributes
            .contains(&("employee_job_level".to_string(), "Lead (003)".to_string()))
    );
    assert!(
        document
            .attributes
            .contains(&("employee_id".to_string(), "WWITest2".to_string()))
    );

    // employeeManager: the entity value is the manager name.
    let manager_cols = vec![
        "Employee ID".to_string(),
        "Name".to_string(),
        "Manager Name".to_string(),
        "From".to_string(),
        "To".to_string(),
        "Event".to_string(),
        "Sub Event".to_string(),
    ];
    let manager_row = serde_json::json!([
        "WWITest2",
        "Dummy1 Test2",
        "Tanmay Dattani (WW2306)",
        "06-07-2025",
        "Present",
        "",
        ""
    ]);
    let document = mappers::format_org_master_table_item(
        &manager_cols,
        manager_row.as_array().unwrap(),
        "employee_manager",
    );
    assert!(
        document
            .body
            .contains("Manager Name: Tanmay Dattani (WW2306)")
    );
    assert!(document.attributes.contains(&(
        "employee_manager".to_string(),
        "Tanmay Dattani (WW2306)".to_string()
    )));
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

/// Employee master fixture: manager EMP001 with reports EMP002/EMP003 plus
/// non-reports EMP004 (other manager) and EMP005 (no manager id).
fn manager_employee_master() -> serde_json::Value {
    json!({
        "status": 1,
        "message": "ok",
        "employee_data": [
            {"employee_id": "EMP001", "company_email_id": "mgr@example.com", "direct_manager_employee_id": "EMP000"},
            {"employee_id": "EMP002", "company_email_id": "r1@example.com", "direct_manager_employee_id": "EMP001"},
            {"employee_id": "EMP003", "company_email_id": "r2@example.com", "direct_manager_employee_id": " EMP001 "},
            {"employee_id": "EMP004", "company_email_id": "o1@example.com", "direct_manager_employee_id": "EMP999"},
            {"employee_id": "EMP005", "company_email_id": "o2@example.com"}
        ]
    })
}

async fn mock_employee_master(server: &MockServer, body: serde_json::Value) {
    Mock::given(method("POST"))
        .and(path("/masterapi/employee"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(server)
        .await;
}

/// A sample leaveActionTakenLeaves payload in the documented provider shape.
fn leave_requests_payload() -> serde_json::Value {
    json!({
        "status": 1,
        "message": "Successfully Loaded All Leaves",
        "data": [
            {
                "id": "T1",
                "applied_leave_id": "AL1",
                "employee_name": "Report One",
                "company_name": "WeWork",
                "employee_no": "EMP002",
                "leave_name": "Privileged Leave",
                "leave_sub_category": "Annual",
                "from": "01-06-2026",
                "to": "03-06-2026",
                "is_unpaid": 0,
                "is_half_day": 0,
                "is_firsthalf_secondhalf": null,
                "fullday_halfday_status": "All Full Days",
                "message": "Family trip",
                "manager_message": "Approved",
                "leave_reason": "Planned leave",
                "action_on": "01-06-2026 10:00:00",
                "action_by": "Mgr (EMP001)",
                "total_working_days": 3,
                "leave_days": ["2026-06-01"],
                "salary": "SECRET-SALARY"
            }
        ]
    })
}

async fn response_body(response: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .expect("response body should be readable");
    serde_json::from_slice(&bytes).expect("response should be valid JSON")
}

#[tokio::test]
async fn get_team_leave_calendar_is_scoped_to_direct_reports() {
    let server = MockServer::start().await;
    mock_employee_master(&server, manager_employee_master()).await;
    Mock::given(method("POST"))
        .and(path("/leavesactionapi/leaveActionTakenLeaves"))
        .respond_with(ResponseTemplate::new(200).set_body_json(leave_requests_payload()))
        .mount(&server)
        .await;

    let config = json!({
        "base_url": server.uri(),
        "read_only": true,
        "authorization": { "participant_mode": "all", "max_batch_size": 20 }
    });
    let response = execute_action(
        "get_team_leave_calendar",
        json!({ "from": "2026-06-01", "to": "2026-06-30" }),
        Some(test_credential()),
        Some(test_source(config)),
        test_actor("mgr@example.com"),
    )
    .await
    .unwrap();
    let body = response_body(response).await;
    assert_eq!(body["status"], json!("success"));
    let item = &body["result"]["data"][0];
    assert_eq!(item["employee_no"], json!("EMP002"));
    assert_eq!(item["leave_name"], json!("Privileged Leave"));
    assert_eq!(item["from"], json!("01-06-2026"));
    assert_eq!(item["total_working_days"], json!(3));
    assert_eq!(item["action_by"], json!("Mgr (EMP001)"));
    assert!(body.to_string().contains("SECRET") == false);

    // The provider request must carry exactly the caller's direct reports and
    // the team-calendar action filter.
    let requests = server
        .received_requests()
        .await
        .expect("mock server should record requests")
        .into_iter()
        .filter(|request| request.url.path() == "/leavesactionapi/leaveActionTakenLeaves")
        .collect::<Vec<_>>();
    assert_eq!(requests.len(), 1);
    let sent: serde_json::Value =
        serde_json::from_slice(&requests[0].body).expect("request body should be JSON");
    assert_eq!(sent["employee_no"], json!(["EMP002", "EMP003"]));
    assert_eq!(sent["action"], json!("2"));
    assert_eq!(sent["from"], json!("2026-06-01"));
    assert_eq!(sent["to"], json!("2026-06-30"));
}

#[tokio::test]
async fn list_pending_leave_approvals_uses_action_one_for_reports() {
    let server = MockServer::start().await;
    mock_employee_master(&server, manager_employee_master()).await;
    Mock::given(method("POST"))
        .and(path("/leavesactionapi/leaveActionTakenLeaves"))
        .respond_with(ResponseTemplate::new(200).set_body_json(leave_requests_payload()))
        .mount(&server)
        .await;

    let config = json!({
        "base_url": server.uri(),
        "read_only": true,
        "authorization": { "participant_mode": "all", "max_batch_size": 20 }
    });
    let response = execute_action(
        "list_pending_leave_approvals",
        json!({}),
        Some(test_credential()),
        Some(test_source(config)),
        test_actor("mgr@example.com"),
    )
    .await
    .unwrap();
    let body = response_body(response).await;
    assert_eq!(body["status"], json!("success"));
    assert_eq!(body["result"]["data"][0]["employee_no"], json!("EMP002"));

    let requests = server
        .received_requests()
        .await
        .expect("mock server should record requests")
        .into_iter()
        .filter(|request| request.url.path() == "/leavesactionapi/leaveActionTakenLeaves")
        .collect::<Vec<_>>();
    assert_eq!(requests.len(), 1);
    let sent: serde_json::Value =
        serde_json::from_slice(&requests[0].body).expect("request body should be JSON");
    assert_eq!(sent["employee_no"], json!(["EMP002", "EMP003"]));
    assert_eq!(sent["action"], json!("1"));
    assert!(sent.get("from").is_none());
    assert!(sent.get("to").is_none());
}

#[tokio::test]
async fn manager_action_requires_direct_reports() {
    let server = MockServer::start().await;
    mock_employee_master(
        &server,
        json!({
            "status": 1,
            "employee_data": [
                {"employee_id": "EMP001", "company_email_id": "mgr@example.com"},
                {"employee_id": "EMP002", "company_email_id": "r1@example.com", "direct_manager_employee_id": "EMP999"}
            ]
        }),
    )
    .await;

    let config = json!({
        "base_url": server.uri(),
        "read_only": true,
        "authorization": { "participant_mode": "all" }
    });
    let error = execute_action(
        "get_team_leave_calendar",
        json!({ "from": "2026-06-01", "to": "2026-06-30" }),
        Some(test_credential()),
        Some(test_source(config)),
        test_actor("mgr@example.com"),
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("no direct reports"));
}

#[tokio::test]
async fn approve_leave_request_rejects_non_report_target() {
    let server = MockServer::start().await;
    mock_employee_master(&server, manager_employee_master()).await;

    // Writes require a non-read-only source.
    let config = json!({
        "base_url": server.uri(),
        "read_only": false,
        "authorization": { "participant_mode": "all" }
    });
    let error = execute_action(
        "approve_leave_request",
        json!({ "employee_no": "EMP004", "leave_id": "L1" }),
        Some(test_credential()),
        Some(test_source(config)),
        test_actor("mgr@example.com"),
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("not a direct report"));
}

#[tokio::test]
async fn approve_leave_request_submits_for_direct_report() {
    let server = MockServer::start().await;
    mock_employee_master(&server, manager_employee_master()).await;
    Mock::given(method("POST"))
        .and(path("/leavesactionapi/leaveaction"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "status": 1,
            "message": "Leave approved"
        })))
        .mount(&server)
        .await;

    let config = json!({
        "base_url": server.uri(),
        "read_only": false,
        "authorization": { "participant_mode": "all" }
    });
    let response = execute_action(
        "approve_leave_request",
        json!({ "employee_no": "EMP002", "leave_id": "T1", "manager_message": "Enjoy" }),
        Some(test_credential()),
        Some(test_source(config)),
        test_actor("mgr@example.com"),
    )
    .await
    .unwrap();
    let body = response_body(response).await;
    assert_eq!(body["status"], json!("success"));
    assert_eq!(body["result"]["status"], json!("submitted"));
    assert_eq!(body["result"]["action"], json!("approve_leave_request"));

    // The decision request must target the report's leave and carry the
    // Darwinbox decision verb and the manager's message.
    let requests = server
        .received_requests()
        .await
        .expect("mock server should record requests")
        .into_iter()
        .filter(|request| request.url.path() == "/leavesactionapi/leaveaction")
        .collect::<Vec<_>>();
    assert_eq!(requests.len(), 1);
    let sent: serde_json::Value =
        serde_json::from_slice(&requests[0].body).expect("request body should be JSON");
    assert_eq!(sent["employee_no"], json!("EMP002"));
    assert_eq!(sent["leave_id"], json!("T1"));
    assert_eq!(sent["action"], json!("approve"));
    assert_eq!(sent["manager_message"], json!("Enjoy"));
}

#[tokio::test]
async fn reject_leave_request_is_blocked_by_read_only_source() {
    let server = MockServer::start().await;
    mock_employee_master(&server, manager_employee_master()).await;

    // test_config defaults to read_only; write actions must refuse loudly.
    // Use an open participant mode so the read-only gate is what rejects.
    let config = json!({
        "base_url": server.uri(),
        "read_only": true,
        "authorization": { "participant_mode": "all" }
    });
    let error = execute_action(
        "reject_leave_request",
        json!({ "employee_no": "EMP002", "leave_id": "T1" }),
        Some(test_credential()),
        Some(test_source(config)),
        test_actor("mgr@example.com"),
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("read-only"));
}

#[tokio::test]
async fn get_my_pending_tasks_resolves_caller_and_returns_task_payload() {
    let server = MockServer::start().await;
    mock_employee_master(&server, manager_employee_master()).await;
    Mock::given(method("POST"))
        .and(path("/orgmasterapi/getTasks"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "status": 1,
            "message": "success",
            "data": {
                "total_count": 2,
                "category_wise_count": { "policy_sign_off": 2 },
                "tasks_data": {
                    "policy_sign_off": {
                        "count": 2,
                        "category_header": "HR Policy Sign Off",
                        "details": {
                            "1": {
                                "id": "task-1",
                                "title": "Please sign-off the policy by clicking on ACT button.",
                                "category": "policy_sign_off",
                                "headers_data": {
                                    "Policy Name": "Code Of Conduct - Section 8",
                                    "Trigger Date with time zone": "05-May-2026",
                                    "Is Overdue": false
                                },
                                "action_buttons": { "1": "ACT" },
                                "user_id": "226875",
                                "mobile_allowed": true,
                                "reportee_task": false,
                                "salary": "SECRET-SALARY"
                            }
                        }
                    }
                }
            }
        })))
        .mount(&server)
        .await;

    let config = json!({
        "base_url": server.uri(),
        "read_only": true,
        "authorization": { "participant_mode": "all" }
    });
    let response = execute_action(
        "get_my_pending_tasks",
        json!({}),
        Some(test_credential()),
        Some(test_source(config)),
        test_actor("mgr@example.com"),
    )
    .await
    .unwrap();
    let body = response_body(response).await;
    assert_eq!(body["status"], json!("success"));
    let task = &body["result"]["data"]["tasks_data"]["policy_sign_off"]["details"]["1"];
    assert_eq!(
        task["title"],
        json!("Please sign-off the policy by clicking on ACT button.")
    );
    assert_eq!(
        task["headers_data"]["Policy Name"],
        json!("Code Of Conduct - Section 8")
    );
    assert_eq!(body["result"]["data"]["total_count"], json!(2));
    assert!(body.to_string().contains("SECRET") == false);

    // The provider request must carry exactly the caller's employee id.
    let requests = server
        .received_requests()
        .await
        .expect("mock server should record requests")
        .into_iter()
        .filter(|request| request.url.path() == "/orgmasterapi/getTasks")
        .collect::<Vec<_>>();
    assert_eq!(requests.len(), 1);
    let sent: serde_json::Value =
        serde_json::from_slice(&requests[0].body).expect("request body should be JSON");
    assert_eq!(sent["employee_id"], json!("EMP001"));
}

#[tokio::test]
async fn get_my_pending_tasks_rejects_identity_spoofing() {
    let server = MockServer::start().await;
    mock_employee_master(&server, manager_employee_master()).await;

    let config = json!({
        "base_url": server.uri(),
        "read_only": true,
        "authorization": { "participant_mode": "all" }
    });
    let error = execute_action(
        "get_my_pending_tasks",
        json!({ "employee_id": "EMP999" }),
        Some(test_credential()),
        Some(test_source(config)),
        test_actor("mgr@example.com"),
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("employee_id"));
}

#[tokio::test]
async fn find_employee_works_for_non_employee_caller() {
    // find_employee is a public directory lookup: a caller who is not a
    // Darwinbox employee (e.g. an admin without a synced profile) must still
    // be able to search the directory. Previously it was gated on the caller
    // resolving to an employee, which blocked non-employees entirely.
    let server = MockServer::start().await;
    mock_employee_master(&server, manager_employee_master()).await;

    let config = json!({
        "base_url": server.uri(),
        "read_only": true,
        "authorization": { "participant_mode": "all" }
    });
    let response = execute_action(
        "find_employee",
        json!({ "query": "r1@example.com" }),
        Some(test_credential()),
        Some(test_source(config)),
        test_actor("praveen@example.com"),
    )
    .await
    .unwrap();
    let body = response_body(response).await;
    assert_eq!(body["status"], json!("success"));
    let employees = body["result"]["employees"]
        .as_array()
        .expect("expected an employees array");
    assert!(!employees.is_empty(), "expected at least one match");
    assert_eq!(employees[0]["employee_id"], json!("EMP002"));
}
