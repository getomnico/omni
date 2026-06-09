mod common;

use anyhow::Result;
use omni_google_connector::models::WebhookNotification;
use shared::db::repositories::SyncRunRepository;
use shared::models::SyncStatus;
use std::sync::atomic::Ordering;
use std::time::Duration;

use common::GoogleConnectorTestFixture;

#[test]
fn test_modification_time_comparison_logic() {
    struct TestCase {
        stored_time: Option<&'static str>,
        current_time: &'static str,
        should_process: bool,
        description: &'static str,
    }

    let test_cases = vec![
        TestCase {
            stored_time: None,
            current_time: "2023-01-01T12:00:00Z",
            should_process: true,
            description: "New file should be processed",
        },
        TestCase {
            stored_time: Some("2023-01-01T12:00:00Z"),
            current_time: "2023-01-01T12:00:00Z",
            should_process: false,
            description: "Unchanged file should be skipped",
        },
        TestCase {
            stored_time: Some("2023-01-01T12:00:00Z"),
            current_time: "2023-01-01T13:00:00Z",
            should_process: true,
            description: "Modified file should be processed",
        },
    ];

    for test_case in test_cases {
        let should_process = match test_case.stored_time {
            Some(stored) => stored != test_case.current_time,
            None => true,
        };

        assert_eq!(
            should_process, test_case.should_process,
            "Failed: {}",
            test_case.description
        );
    }
}

// ============================================================================
// Webhook debounce tests
// ============================================================================

#[tokio::test]
async fn test_webhook_debounce_buffers_and_flushes() -> Result<()> {
    let fixture = GoogleConnectorTestFixture::new().await?;
    let source_id = fixture.source_id().to_string();

    // Set debounce to zero so entries expire immediately
    fixture
        .sync_manager
        .debounce_duration_ms
        .store(0, Ordering::Relaxed);

    let states = ["add", "update", "change", "update", "remove"];
    for state in &states {
        let notification = WebhookNotification {
            channel_id: "ch-1".to_string(),
            resource_state: state.to_string(),
            resource_id: Some("res-1".to_string()),
            resource_uri: None,
            changed: None,
            source_id: Some(source_id.clone()),
        };
        fixture
            .sync_manager
            .handle_webhook_notification(notification)
            .await?;
    }

    // All 5 webhooks should be buffered into a single debounce entry
    assert_eq!(fixture.sync_manager.webhook_debounce.len(), 1);
    let entry = fixture
        .sync_manager
        .webhook_debounce
        .get(&source_id)
        .expect("debounce entry should exist");
    assert_eq!(entry.count, 5);
    drop(entry);

    // Spawn the processor briefly — with Duration::ZERO the entry is already expired
    let sm = fixture.sync_manager.clone();
    let processor = tokio::spawn(async move {
        sm.run_webhook_processor().await;
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    processor.abort();

    // End-to-end: webhook → CM → POST /sync on the real SDK-served connector
    // → GoogleConnector::sync → run_sync → credentials lookup fails (no creds
    // seeded in the test DB) → SDK returns 4xx/5xx → CM's connector_client
    // surfaces that as ClientError → CM marks the sync_run failed. We assert
    // the terminal state rather than just the presence of a running row so a
    // regression that silently drops the sync (or hangs it) fails this test.
    let sync_run_repo = SyncRunRepository::new(fixture.pool());
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let terminal_run = loop {
        let latest = sync_run_repo
            .find_latest_for_sources(&[source_id.clone()])
            .await?
            .into_iter()
            .next();
        if let Some(run) = latest {
            if run.status != SyncStatus::Running {
                break run;
            }
        }
        if tokio::time::Instant::now() >= deadline {
            panic!("no terminal sync run for source {} within 5s", source_id);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };

    assert_eq!(
        terminal_run.status,
        SyncStatus::Failed,
        "sync run should have failed (no credentials seeded)"
    );
    assert!(
        terminal_run.error_message.is_some(),
        "failed sync run should record an error message"
    );

    Ok(())
}

#[tokio::test]
async fn test_webhook_debounce_retains_unexpired() -> Result<()> {
    let fixture = GoogleConnectorTestFixture::new().await?;
    let source_id = fixture.source_id().to_string();

    // Set debounce to 1 hour so entries never expire during this test
    fixture
        .sync_manager
        .debounce_duration_ms
        .store(3_600_000, Ordering::Relaxed);

    let notification = WebhookNotification {
        channel_id: "ch-2".to_string(),
        resource_state: "update".to_string(),
        resource_id: Some("res-2".to_string()),
        resource_uri: None,
        changed: None,
        source_id: Some(source_id.clone()),
    };
    fixture
        .sync_manager
        .handle_webhook_notification(notification)
        .await?;

    // Spawn processor briefly
    let sm = fixture.sync_manager.clone();
    let processor = tokio::spawn(async move {
        sm.run_webhook_processor().await;
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    processor.abort();

    // Entry should still be in the debounce map (not expired)
    assert_eq!(
        fixture.sync_manager.webhook_debounce.len(),
        1,
        "debounce entry should be retained when not yet expired"
    );

    // No sync run should have been created
    let sync_run_repo = SyncRunRepository::new(fixture.pool());
    let running = sync_run_repo.get_running_for_source(&source_id).await?;
    assert!(
        running.is_none(),
        "no sync run should be created for unexpired debounce entry"
    );

    Ok(())
}

// ============================================================================
// Drive buffer memory budget tests
// ============================================================================

mod drive_buffer_budget_tests {
    use anyhow::Result;
    use axum::{
        extract::{Path, Query, State},
        response::Json,
        routing::{get, post, put},
        Router,
    };
    use omni_connector_sdk::{
        AuthType, SdkClient, ServiceCredential, ServiceProvider, Source, SourceType, SyncContext,
        SyncType,
    };
    use omni_google_connector::{admin::AdminClient, sync::SyncManager};
    use serde_json::{json, Value as JsonValue};
    use shared::models::{SourceScope, UserFilterMode};
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;
    use time::OffsetDateTime;
    use tokio::net::TcpListener;

    const SOURCE_ID: &str = "google-drive-budget-source";
    const SYNC_RUN_ID: &str = "google-drive-budget-sync";
    const USER_EMAIL: &str = "user@example.com";
    const MIB: usize = 1024 * 1024;
    const BUDGET_BYTES: usize = 512 * MIB;

    static DRIVE_ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    #[derive(Clone)]
    struct MockDriveFile {
        id: &'static str,
        name: &'static str,
        declared_size: usize,
    }

    const MOCK_FILES: &[MockDriveFile] = &[
        MockDriveFile {
            id: "file-256",
            name: "256mb.txt",
            declared_size: 256 * MIB,
        },
        MockDriveFile {
            id: "file-128-a",
            name: "128mb-a.txt",
            declared_size: 128 * MIB,
        },
        MockDriveFile {
            id: "file-64",
            name: "64mb.txt",
            declared_size: 64 * MIB,
        },
        MockDriveFile {
            id: "file-32",
            name: "32mb.txt",
            declared_size: 32 * MIB,
        },
        MockDriveFile {
            id: "file-400",
            name: "400mb.txt",
            declared_size: 400 * MIB,
        },
        MockDriveFile {
            id: "file-300",
            name: "300mb.txt",
            declared_size: 300 * MIB,
        },
        MockDriveFile {
            id: "file-96",
            name: "96mb.txt",
            declared_size: 96 * MIB,
        },
        MockDriveFile {
            id: "file-128-b",
            name: "128mb-b.txt",
            declared_size: 128 * MIB,
        },
    ];

    #[derive(Clone, Default)]
    struct MockDriveState {
        active_downloads: Arc<AtomicUsize>,
        max_active_downloads: Arc<AtomicUsize>,
        active_declared_bytes: Arc<AtomicUsize>,
        max_active_declared_bytes: Arc<AtomicUsize>,
        budget_breached: Arc<AtomicBool>,
    }

    impl MockDriveState {
        fn enter_download(&self, declared_size: usize) {
            let active_downloads = self.active_downloads.fetch_add(1, Ordering::SeqCst) + 1;
            self.max_active_downloads
                .fetch_max(active_downloads, Ordering::SeqCst);

            let active_bytes = self
                .active_declared_bytes
                .fetch_add(declared_size, Ordering::SeqCst)
                + declared_size;
            self.max_active_declared_bytes
                .fetch_max(active_bytes, Ordering::SeqCst);
            if active_bytes > BUDGET_BYTES {
                self.budget_breached.store(true, Ordering::SeqCst);
            }
        }

        fn exit_download(&self, declared_size: usize) {
            self.active_declared_bytes
                .fetch_sub(declared_size, Ordering::SeqCst);
            self.active_downloads.fetch_sub(1, Ordering::SeqCst);
        }
    }

    async fn spawn_mock_drive() -> Result<(String, MockDriveState)> {
        let state = MockDriveState::default();
        let app = Router::new()
            .route("/drive/v3/files", get(list_files))
            .route("/drive/v3/files/:file_id", get(get_file_or_media))
            .route("/drive/v3/changes/startPageToken", get(start_page_token))
            .with_state(state.clone());

        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        tokio::spawn(async move {
            axum::serve(listener, app).await.ok();
        });

        Ok((format!("http://{}", addr), state))
    }

    async fn list_files() -> Json<JsonValue> {
        let files: Vec<JsonValue> = MOCK_FILES
            .iter()
            .map(|file| {
                json!({
                    "id": file.id,
                    "name": file.name,
                    "mimeType": "text/plain",
                    "size": file.declared_size.to_string(),
                    "webViewLink": format!("https://example.test/{}", file.id),
                    "createdTime": "2024-01-01T00:00:00Z",
                    "modifiedTime": "2024-01-01T00:00:00Z"
                })
            })
            .collect();

        Json(json!({ "files": files }))
    }

    async fn get_file_or_media(
        State(state): State<MockDriveState>,
        Path(file_id): Path<String>,
        Query(query): Query<HashMap<String, String>>,
    ) -> Json<JsonValue> {
        if query.get("alt").map(String::as_str) == Some("media") {
            let declared_size = MOCK_FILES
                .iter()
                .find(|file| file.id == file_id)
                .map(|file| file.declared_size)
                .expect("mock file id should exist");

            state.enter_download(declared_size);
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
            state.exit_download(declared_size);
            return Json(json!(format!("content for {}", file_id)));
        }

        Json(json!({
            "id": file_id,
            "name": "metadata.txt",
            "mimeType": "text/plain"
        }))
    }

    async fn start_page_token() -> Json<JsonValue> {
        Json(json!({"startPageToken": "next-page-token"}))
    }

    async fn spawn_mock_connector_manager() -> Result<String> {
        let app = Router::new()
            .route("/sdk/connector-configs/:provider", get(connector_config))
            .route("/sdk/content", post(store_content))
            .route("/sdk/events/batch", post(ok_json))
            .route("/sdk/sync/:sync_run_id/scanned", post(ok_json))
            .route("/sdk/sync/:sync_run_id/updated", post(ok_json))
            .route("/sdk/sync/:sync_run_id/complete", post(ok_json))
            .route("/sdk/source/:source_id/connector-state", put(ok_json));

        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        tokio::spawn(async move {
            axum::serve(listener, app).await.ok();
        });

        Ok(format!("http://{}", addr))
    }

    async fn connector_config() -> Json<JsonValue> {
        Json(json!({
            "oauth_client_id": "test-client-id",
            "oauth_client_secret": "test-client-secret"
        }))
    }

    async fn store_content() -> Json<JsonValue> {
        Json(json!({"content_id": "content-id"}))
    }

    async fn ok_json() -> Json<JsonValue> {
        Json(json!({}))
    }

    fn test_source() -> Source {
        let now = OffsetDateTime::now_utc();
        Source {
            id: SOURCE_ID.to_string(),
            name: "Google Drive Budget Test".to_string(),
            source_type: SourceType::GoogleDrive,
            config: json!({}),
            is_active: true,
            is_deleted: false,
            scope: SourceScope::User,
            user_filter_mode: UserFilterMode::All,
            user_whitelist: None,
            user_blacklist: None,
            connector_state: None,
            checkpoint: None,
            sync_interval_seconds: None,
            created_at: now,
            updated_at: now,
            created_by: "user-id".to_string(),
        }
    }

    fn oauth_credentials() -> ServiceCredential {
        let now = OffsetDateTime::now_utc();
        ServiceCredential {
            id: "credential-id".to_string(),
            source_id: SOURCE_ID.to_string(),
            user_id: Some("user-id".to_string()),
            provider: ServiceProvider::Google,
            auth_type: AuthType::OAuth,
            principal_email: Some(USER_EMAIL.to_string()),
            credentials: json!({
                "access_token": "access-token",
                "refresh_token": "refresh-token",
                "expires_at": now.unix_timestamp() + 3600,
                "user_email": USER_EMAIL
            }),
            config: json!({}),
            expires_at: None,
            last_validated_at: None,
            created_at: now,
            updated_at: now,
        }
    }

    #[tokio::test]
    async fn drive_buffer_budget_never_exceeds_declared_in_flight_bytes() -> Result<()> {
        let _env_guard = DRIVE_ENV_LOCK.lock().await;
        let (drive_base_url, drive_state) = spawn_mock_drive().await?;
        let previous_drive_base = std::env::var("GOOGLE_DRIVE_API_BASE").ok();
        std::env::set_var(
            "GOOGLE_DRIVE_API_BASE",
            format!("{}/drive/v3", drive_base_url),
        );

        let cm_url = spawn_mock_connector_manager().await?;
        let sdk_client = SdkClient::new(&cm_url);
        sdk_client.register_sync(SYNC_RUN_ID, SyncType::Full).await;

        let sync_manager = SyncManager::new(Arc::new(AdminClient::new()), sdk_client.clone(), None);
        let ctx = SyncContext::new(
            sdk_client,
            SYNC_RUN_ID.to_string(),
            SOURCE_ID.to_string(),
            SourceType::GoogleDrive,
            SyncType::Full,
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
        );

        let sync_result = sync_manager
            .run_sync(test_source(), Some(oauth_credentials()), None, ctx)
            .await;

        if let Some(value) = previous_drive_base {
            std::env::set_var("GOOGLE_DRIVE_API_BASE", value);
        } else {
            std::env::remove_var("GOOGLE_DRIVE_API_BASE");
        }

        sync_result?;

        assert!(
            drive_state.max_active_downloads.load(Ordering::SeqCst) > 1,
            "test should exercise concurrent downloads for smaller files"
        );
        assert!(
            !drive_state.budget_breached.load(Ordering::SeqCst),
            "declared in-flight download bytes breached the 512 MiB budget"
        );
        assert!(
            drive_state.max_active_declared_bytes.load(Ordering::SeqCst) <= BUDGET_BYTES,
            "max declared in-flight bytes ({}) exceeded budget ({})",
            drive_state.max_active_declared_bytes.load(Ordering::SeqCst),
            BUDGET_BYTES
        );

        Ok(())
    }
}
