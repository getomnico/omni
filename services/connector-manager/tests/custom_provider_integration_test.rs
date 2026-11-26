mod common;

use axum::http::StatusCode;
use axum_test::{TestServer, TestServerConfig};
use serde_json::json;
use shared::db::repositories::ServiceCredentialsRepo;
use shared::models::{AuthType, ConnectorManifest, IntegrationType, ServiceCredential, SyncType};
use time::OffsetDateTime;

#[tokio::test]
async fn registered_custom_provider_credentials_reach_sync_config_and_dispatch() {
    let fixture = common::setup_test_fixture().await.unwrap();
    let server = TestServer::new_with_config(
        fixture.app.clone(),
        TestServerConfig::builder()
            .default_content_type("application/json")
            .expect_success_by_default()
            .build(),
    )
    .unwrap();
    let connector = &fixture.mock_connector;
    let manifest = ConnectorManifest {
        name: "Custom connector".into(),
        display_name: "Custom connector".into(),
        version: "1".into(),
        sync_modes: vec![SyncType::Full],
        connector_id: "custom-acme".into(),
        connector_url: connector.base_url.clone(),
        integration_type: IntegrationType::Connector,
        source_types: vec!["acme_crm".into()],
        description: None,
        actions: vec![],
        search_operators: vec![],
        read_only: false,
        extra_schema: None,
        attributes_schema: None,
        mcp_enabled: false,
        mcp_catalog_loaded: false,
        prompts: vec![],
        skills: vec![],
        source_capabilities: vec![],
        resources: vec![],
        oauth: None,
    };
    connector.set_manifest(serde_json::to_value(&manifest).unwrap());
    server
        .post("/sdk/register")
        .json(&manifest)
        .await
        .assert_status(StatusCode::OK);

    let pool = fixture.state.db_pool.pool();
    let source_id = shared::utils::generate_ulid();
    let user_id = "01JGF7V3E0Y2R1X8P5Q7W9T4N6";
    sqlx::query("INSERT INTO sources (id,name,source_type,integration_type,config,is_active,created_by,created_at,updated_at) VALUES ($1,'Acme CRM','acme_crm','connector',$2,true,$3,NOW(),NOW())")
        .bind(&source_id).bind(json!({"region":"west"})).bind(user_id).execute(pool).await.unwrap();
    ServiceCredentialsRepo::new(pool.clone())
        .unwrap()
        .create(ServiceCredential {
            id: shared::utils::generate_ulid(),
            source_id: source_id.clone(),
            user_id: None,
            provider: "acme_crm".into(),
            auth_type: AuthType::ApiKey,
            principal_email: None,
            credentials: json!({"api_key":"secret"}),
            config: json!({}),
            expires_at: None,
            last_validated_at: None,
            created_at: OffsetDateTime::now_utc(),
            updated_at: OffsetDateTime::now_utc(),
        })
        .await
        .unwrap();

    let config: serde_json::Value = server
        .get(&format!("/sdk/source/{source_id}/sync-config"))
        .await
        .json();
    assert_eq!(config["source_type"], "acme_crm");
    assert_eq!(config["config"]["region"], "west");
    assert_eq!(config["credentials"], json!({}));

    fixture
        .state
        .sync_manager
        .trigger_sync(
            &source_id,
            SyncType::Full,
            omni_connector_manager::sync_manager::TriggerType::Manual,
        )
        .await
        .unwrap();
    let requests = connector.get_sync_requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].source_id, source_id);
}
