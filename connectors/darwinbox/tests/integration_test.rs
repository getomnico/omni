use omni_connector_sdk::{AuthType, ServiceCredential, ServiceProvider};
use omni_darwinbox_connector::actions::execute_action;
use omni_darwinbox_connector::client::DarwinboxClient;
use omni_darwinbox_connector::config::DarwinboxSourceConfig;
use omni_darwinbox_connector::credentials::DarwinboxCredentials;
use serde_json::json;
use time::OffsetDateTime;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

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

#[tokio::test]
async fn client_fetch_employees_injects_dataset_and_api_key() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/masterapi/employee"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "status": 1,
            "message": "ok",
            "employee_data": [{
                "employee_id": "EMP001",
                "first_name": "Asha",
                "last_name": "Rao",
                "company_email_id": "asha@example.com"
            }]
        })))
        .mount(&server)
        .await;

    let config = DarwinboxSourceConfig {
        base_url: server.uri(),
        default_timezone: None,
        sync_modules: Default::default(),
        action_modules: Default::default(),
        authorization: Default::default(),
    };
    let credentials = DarwinboxCredentials::Basic {
        username: "api-user".to_string(),
        password: "secret".to_string(),
        api_key: "api-key".to_string(),
        dataset_key: "dataset-key".to_string(),
    };
    let client = DarwinboxClient::new(&config, credentials).unwrap();
    let response = client.fetch_employees(None, None).await.unwrap();

    assert_eq!(response.employee_data.len(), 1);
    assert_eq!(
        response.employee_data[0].employee_id.as_deref(),
        Some("EMP001")
    );
}

#[tokio::test]
async fn my_actions_reject_spoofed_employee_identity_fields() {
    let params = json!({
        "base_url": "https://example.darwinbox.in",
        "_omni_action_context": {
            "actor": {
                "actor_type": "user",
                "user_id": "user-a",
                "email": "a@example.com",
                "role": "User"
            }
        },
        "employee_no": "EMP-B"
    });

    let error = execute_action("get_my_leave_balance", params, Some(test_credential()))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("employee_no"));
}
