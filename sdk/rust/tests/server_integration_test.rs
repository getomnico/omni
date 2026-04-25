mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use axum::{routing, Router};
use axum_test::{TestServer, TestServerConfig};
use common::{GetSourceBehavior, MockConnectorManager, SyncBehavior, TestConnector};
use omni_connector_sdk::{
    create_router, Connector, ServiceCredentials, Source, SourceType, SyncContext,
};
use serde_json::{json, Value as JsonValue};
use tokio::sync::Notify;

const CONNECTOR_URL: &str = "http://test-connector";

fn build_server<C>(connector: Arc<C>, mock: &MockConnectorManager) -> TestServer
where
    C: Connector + 'static,
{
    build_server_with_extra(connector, mock, Router::new())
}

fn build_server_with_extra<C>(
    connector: Arc<C>,
    mock: &MockConnectorManager,
    extra: Router,
) -> TestServer
where
    C: Connector + 'static,
{
    let router =
        create_router(connector, mock.sdk_client(), CONNECTOR_URL.to_string()).merge(extra);
    let config = TestServerConfig::builder()
        .default_content_type("application/json")
        .build();
    TestServer::new_with_config(router, config).unwrap()
}

/// Poll `active_syncs` indirectly: a second `/sync` for the same source
/// returns 200 when the slot is free, 409 while it's reserved.
async fn wait_for_slot_release(server: &TestServer, sync_run_id: &str, source_id: &str) {
    for _ in 0..40 {
        let resp = server
            .post("/sync")
            .json(&json!({
                "sync_run_id": sync_run_id,
                "source_id": source_id,
                "sync_mode": "full",
            }))
            .await;
        if resp.status_code() == 200 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("slot never released");
}

#[tokio::test]
async fn t1_manifest_endpoint_returns_connector_metadata() -> Result<()> {
    let mock = MockConnectorManager::spawn().await;
    let connector = Arc::new(TestConnector::new(SyncBehavior::Ok));
    let server = build_server(connector, &mock);

    let resp = server.get("/manifest").await;
    let body: serde_json::Value = resp.json();
    assert_eq!(body["name"], "test");
    assert_eq!(body["version"], "0.0.0");
    assert_eq!(body["connector_url"], CONNECTOR_URL);
    Ok(())
}

#[tokio::test]
async fn t2_sync_returns_409_when_source_already_syncing() -> Result<()> {
    let mock = MockConnectorManager::spawn().await;
    mock.set_source(json!({}));

    let notify = Arc::new(Notify::new());
    let connector = Arc::new(TestConnector::new(SyncBehavior::BlockUntil(Arc::clone(
        &notify,
    ))));
    let server = build_server(Arc::clone(&connector), &mock);

    let resp = server
        .post("/sync")
        .json(&json!({
            "sync_run_id": "run-1",
            "source_id": "src-1",
            "sync_mode": "full",
        }))
        .await;
    assert_eq!(resp.status_code(), 200);

    // Wait until the sync is actually running before firing the second request,
    // otherwise the first `trigger_sync` may still be in the get_source await
    // and hasn't reserved its slot yet.
    for _ in 0..40 {
        if connector.sync_call_count() >= 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    let resp = server
        .post("/sync")
        .json(&json!({
            "sync_run_id": "run-2",
            "source_id": "src-1",
            "sync_mode": "full",
        }))
        .await;
    assert_eq!(resp.status_code(), 409);

    notify.notify_one();
    Ok(())
}

#[tokio::test]
async fn t3_concurrent_sync_requests_for_same_source_only_one_accepted() -> Result<()> {
    // Regression for issue 1a: without an atomic reservation, multiple
    // requests can pass `contains_key` while the previous one is still in the
    // `get_source` await. We bind a real TcpListener and drive requests with
    // reqwest so futures are `Send` and can be spawned in parallel tasks.
    let mock = MockConnectorManager::spawn().await;
    mock.set_source(json!({}));

    let notify = Arc::new(Notify::new());
    let connector = Arc::new(TestConnector::new(SyncBehavior::BlockUntil(Arc::clone(
        &notify,
    ))));
    let router = create_router(connector, mock.sdk_client(), CONNECTOR_URL.to_string());

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let _server = tokio::spawn(async move {
        axum::serve(listener, router).await.ok();
    });

    let client = reqwest::Client::new();
    let url = format!("http://{}/sync", addr);
    let mut handles = Vec::new();
    for i in 0..10 {
        let client = client.clone();
        let url = url.clone();
        handles.push(tokio::spawn(async move {
            client
                .post(&url)
                .json(&json!({
                    "sync_run_id": format!("run-{}", i),
                    "source_id": "src-shared",
                    "sync_mode": "full",
                }))
                .send()
                .await
                .unwrap()
                .status()
                .as_u16()
        }));
    }

    let mut two_hundreds = 0;
    let mut four_oh_nines = 0;
    for handle in handles {
        match handle.await? {
            200 => two_hundreds += 1,
            409 => four_oh_nines += 1,
            other => panic!("unexpected status {}", other),
        }
    }
    assert_eq!(two_hundreds, 1, "exactly one request should win the race");
    assert_eq!(four_oh_nines, 9);

    notify.notify_waiters();
    Ok(())
}

#[tokio::test]
async fn t4_sync_returns_404_when_source_not_found() -> Result<()> {
    // Regression for issue 2 (typed SdkError): previously the server
    // string-matched "404" in the error message, which was brittle.
    let mock = MockConnectorManager::spawn().await;
    mock.set_source_behavior(GetSourceBehavior::NotFound);

    let connector = Arc::new(TestConnector::new(SyncBehavior::Ok));
    let server = build_server(connector, &mock);

    let resp = server
        .post("/sync")
        .json(&json!({
            "sync_run_id": "run-1",
            "source_id": "missing",
            "sync_mode": "full",
        }))
        .await;
    assert_eq!(resp.status_code(), 404);
    Ok(())
}

#[tokio::test]
async fn t5_sync_returns_500_on_upstream_error() -> Result<()> {
    let mock = MockConnectorManager::spawn().await;
    mock.set_source_behavior(GetSourceBehavior::ServerError);

    let connector = Arc::new(TestConnector::new(SyncBehavior::Ok));
    let server = build_server(connector, &mock);

    let resp = server
        .post("/sync")
        .json(&json!({
            "sync_run_id": "run-1",
            "source_id": "src-1",
            "sync_mode": "full",
        }))
        .await;
    assert_eq!(resp.status_code(), 500);
    Ok(())
}

#[tokio::test]
async fn t7_panic_in_sync_clears_active_syncs() -> Result<()> {
    // Regression for issue 1b: a panic inside the spawned sync task must not
    // leak the active_syncs entry. Otherwise the source is wedged at 409.
    let mock = MockConnectorManager::spawn().await;
    mock.set_source(json!({}));

    let connector = Arc::new(TestConnector::new(SyncBehavior::Panic));
    let server = build_server(Arc::clone(&connector), &mock);

    let resp = server
        .post("/sync")
        .json(&json!({
            "sync_run_id": "run-1",
            "source_id": "src-1",
            "sync_mode": "full",
        }))
        .await;
    assert_eq!(resp.status_code(), 200);

    // Switch to Ok so the follow-up succeeds cleanly.
    connector.set_behavior(SyncBehavior::Ok);
    wait_for_slot_release(&server, "run-2", "src-1").await;
    Ok(())
}

#[tokio::test]
async fn t8_cancel_matches_by_sync_run_id() -> Result<()> {
    let mock = MockConnectorManager::spawn().await;
    mock.set_source(json!({}));

    let notify = Arc::new(Notify::new());
    let connector = Arc::new(TestConnector::new(SyncBehavior::BlockUntil(Arc::clone(
        &notify,
    ))));
    let server = build_server(Arc::clone(&connector), &mock);

    let resp = server
        .post("/sync")
        .json(&json!({
            "sync_run_id": "run-to-cancel",
            "source_id": "src-1",
            "sync_mode": "full",
        }))
        .await;
    assert_eq!(resp.status_code(), 200);

    // Wait until the connector is actually running so cancel has something
    // to target.
    for _ in 0..40 {
        if connector.sync_call_count() >= 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    let resp = server
        .post("/cancel")
        .json(&json!({ "sync_run_id": "run-to-cancel" }))
        .await;
    let body: serde_json::Value = resp.json();
    assert_eq!(body["status"], "cancelled");

    // The slot should be released shortly after cancel (BlockUntil observes
    // ctx.is_cancelled() on its next poll).
    wait_for_slot_release(&server, "run-2", "src-1").await;
    Ok(())
}

#[tokio::test]
async fn t9_cancel_returns_not_found_for_unknown_sync() -> Result<()> {
    let mock = MockConnectorManager::spawn().await;
    let connector = Arc::new(TestConnector::new(SyncBehavior::Ok));
    let server = build_server(connector, &mock);

    let resp = server
        .post("/cancel")
        .json(&json!({ "sync_run_id": "does-not-exist" }))
        .await;
    let body: serde_json::Value = resp.json();
    assert_eq!(body["status"], "not_found");
    Ok(())
}

#[tokio::test]
async fn t10_sync_status_returns_running_while_sync_active() -> Result<()> {
    let mock = MockConnectorManager::spawn().await;
    mock.set_source(json!({}));

    let notify = Arc::new(Notify::new());
    let connector = Arc::new(TestConnector::new(SyncBehavior::BlockUntil(Arc::clone(
        &notify,
    ))));
    let server = build_server(Arc::clone(&connector), &mock);

    let resp = server
        .post("/sync")
        .json(&json!({
            "sync_run_id": "run-status",
            "source_id": "src-1",
            "sync_mode": "full",
        }))
        .await;
    assert_eq!(resp.status_code(), 200);

    // Wait until the sync is actually running
    for _ in 0..40 {
        if connector.sync_call_count() >= 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    let resp = server.get("/sync/run-status").await;
    let body: serde_json::Value = resp.json();
    assert_eq!(body["running"], true);

    notify.notify_one();
    wait_for_slot_release(&server, "run-2", "src-1").await;

    let resp = server.get("/sync/run-status").await;
    let body: serde_json::Value = resp.json();
    assert_eq!(body["running"], false);

    Ok(())
}

#[tokio::test]
async fn t11_sync_status_returns_not_running_for_unknown_sync() -> Result<()> {
    let mock = MockConnectorManager::spawn().await;
    let connector = Arc::new(TestConnector::new(SyncBehavior::Ok));
    let server = build_server(connector, &mock);

    let resp = server.get("/sync/never-started").await;
    let body: serde_json::Value = resp.json();
    assert_eq!(body["running"], false);
    Ok(())
}

#[tokio::test]
async fn t12_extra_routes_are_served_alongside_sdk_routes() -> Result<()> {
    let mock = MockConnectorManager::spawn().await;
    let connector = Arc::new(TestConnector::new(SyncBehavior::Ok));
    let extra = Router::new().route("/custom/ping", routing::get(|| async { "pong" }));
    let server = build_server_with_extra(connector, &mock, extra);

    let resp = server.get("/health").await;
    assert_eq!(resp.status_code(), 200);

    let resp = server.get("/custom/ping").await;
    assert_eq!(resp.status_code(), 200);
    assert_eq!(resp.text(), "pong");
    Ok(())
}

// ---------------------------------------------------------------------------
// Action endpoint tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn t13_action_not_supported_returns_404() -> Result<()> {
    let mock = MockConnectorManager::spawn().await;
    let connector = Arc::new(TestConnector::new(SyncBehavior::Ok));
    let server = build_server(connector, &mock);

    let resp = server
        .post("/action")
        .json(&json!({
            "action": "unknown_action",
            "params": {},
            "credentials": {},
        }))
        .await;

    assert_eq!(resp.status_code(), 404);
    let body: serde_json::Value = resp.json();
    assert_eq!(body["status"], "error");
    assert!(body["error"].as_str().unwrap().contains("not supported"));
    Ok(())
}

#[tokio::test]
async fn t14_action_success_returns_200() -> Result<()> {
    let mock = MockConnectorManager::spawn().await;

    // Connector that returns a success response for a known action.
    struct SuccessConnector {
        sync_called: Arc<Mutex<u32>>,
    }

    impl Default for SuccessConnector {
        fn default() -> Self {
            Self {
                sync_called: Arc::new(Mutex::new(0)),
            }
        }
    }

    #[async_trait::async_trait]
    impl Connector for SuccessConnector {
        type State = JsonValue;

        fn name(&self) -> &'static str {
            "success-test"
        }
        fn version(&self) -> &'static str {
            "0.0.1"
        }
        fn source_types(&self) -> Vec<SourceType> {
            vec![SourceType::Web]
        }
        fn requires_credentials(&self) -> bool {
            false
        }

        async fn sync(
            &self,
            _source: Source,
            _credentials: Option<ServiceCredentials>,
            _state: Option<Self::State>,
            _ctx: SyncContext,
        ) -> Result<()> {
            Ok(())
        }

        async fn execute_action(
            &self,
            action: &str,
            _params: JsonValue,
            _credentials: JsonValue,
        ) -> Result<axum::response::Response> {
            use omni_connector_sdk::models::ActionResponse;
            if action == "do_thing" {
                Ok(ActionResponse::success(json!({"ok": true})).into_response())
            } else {
                Ok(ActionResponse::not_supported(action).into_response())
            }
        }
    }

    let connector = Arc::new(SuccessConnector::default());
    let server = build_server(connector, &mock);

    let resp = server
        .post("/action")
        .json(&json!({
            "action": "do_thing",
            "params": {},
            "credentials": {},
        }))
        .await;

    assert_eq!(resp.status_code(), 200);
    let body: serde_json::Value = resp.json();
    assert_eq!(body["status"], "success");
    assert_eq!(body["result"]["ok"], true);
    Ok(())
}

#[tokio::test]
async fn t15_action_exception_returns_500() -> Result<()> {
    let mock = MockConnectorManager::spawn().await;

    // Connector that panics inside execute_action.
    struct PanicConnector {
        sync_called: Arc<Mutex<u32>>,
    }

    impl Default for PanicConnector {
        fn default() -> Self {
            Self {
                sync_called: Arc::new(Mutex::new(0)),
            }
        }
    }

    #[async_trait::async_trait]
    impl Connector for PanicConnector {
        type State = JsonValue;

        fn name(&self) -> &'static str {
            "panic-test"
        }
        fn version(&self) -> &'static str {
            "0.0.1"
        }
        fn source_types(&self) -> Vec<SourceType> {
            vec![SourceType::Web]
        }
        fn requires_credentials(&self) -> bool {
            false
        }

        async fn sync(
            &self,
            _source: Source,
            _credentials: Option<ServiceCredentials>,
            _state: Option<Self::State>,
            _ctx: SyncContext,
        ) -> Result<()> {
            Ok(())
        }

        async fn execute_action(
            &self,
            action: &str,
            _params: JsonValue,
            _credentials: JsonValue,
        ) -> Result<axum::response::Response> {
            if action == "crash_me" {
                return Err(anyhow::anyhow!("intentional action panic"));
            }
            use omni_connector_sdk::models::ActionResponse;
            Ok(ActionResponse::not_supported(action).into_response())
        }
    }

    let connector = Arc::new(PanicConnector::default());
    let server = build_server(connector, &mock);

    let resp = server
        .post("/action")
        .json(&json!({
            "action": "crash_me",
            "params": {},
            "credentials": {},
        }))
        .await;

    assert_eq!(resp.status_code(), 500);
    let body: serde_json::Value = resp.json();
    assert_eq!(body["status"], "error");
    assert!(body["error"]
        .as_str()
        .unwrap()
        .contains("intentional action panic"));
    Ok(())
}
