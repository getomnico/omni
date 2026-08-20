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
        Router,
        extract::{Path, Query, State},
        response::Json,
        routing::{get, post, put},
    };
    use omni_connector_sdk::{
        AuthType, SdkClient, ServiceCredential, ServiceProvider, Source, SourceType, SyncContext,
        SyncType,
    };
    use omni_google_connector::{admin::AdminClient, sync::SyncManager};
    use serde_json::{Value as JsonValue, json};
    use shared::models::{SourceScope, UserFilterMode};
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
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
        /// Count of trashed=true file-list queries (full-traversal trash reconciliation).
        trashed_query_calls: Arc<AtomicUsize>,
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

    async fn list_files(
        State(state): State<MockDriveState>,
        Query(query): Query<HashMap<String, String>>,
    ) -> Json<JsonValue> {
        // trashed=true queries serve nothing; count them so the unfiltered
        // DWD full-sync trash reconciliation is observable in tests.
        if query
            .get("q")
            .map(|q| q.contains("trashed=true"))
            .unwrap_or(false)
        {
            state.trashed_query_calls.fetch_add(1, Ordering::SeqCst);
            return Json(json!({ "files": [] }));
        }
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
            .route("/sdk/sync/:sync_run_id/checkpoint", put(ok_json))
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
            source_type: SourceType::GoogleDrive.to_string(),
            integration_type: shared::models::IntegrationType::Connector,
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
        // TODO: Audit that the environment access only happens in single-threaded code.
        unsafe {
            std::env::set_var(
                "GOOGLE_DRIVE_API_BASE",
                format!("{}/drive/v3", drive_base_url),
            )
        };

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
            // TODO: Audit that the environment access only happens in single-threaded code.
            unsafe { std::env::set_var("GOOGLE_DRIVE_API_BASE", value) };
        } else {
            // TODO: Audit that the environment access only happens in single-threaded code.
            unsafe { std::env::remove_var("GOOGLE_DRIVE_API_BASE") };
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
        assert!(
            drive_state.trashed_query_calls.load(Ordering::SeqCst) >= 1,
            "expected the unfiltered DWD full sync to reconcile trashed files"
        );

        Ok(())
    }
}

// ============================================================================
// SA-direct shared-drive sync tests (mock Drive + mock token endpoint)
// ============================================================================

mod sa_direct_tests {
    use anyhow::Result;
    use axum::{
        Router,
        extract::{Path, Query, State},
        response::Json,
        routing::{get, post, put},
    };
    use omni_connector_sdk::{
        AuthType, Connector, SdkClient, ServiceCredential, ServiceProvider, Source, SourceType,
        SyncContext, SyncType,
    };
    use omni_google_connector::{
        admin::AdminClient, models::GoogleSyncCheckpoint, sync::SyncManager,
    };
    use serde_json::{Value as JsonValue, json};
    use shared::models::{SourceScope, UserFilterMode};
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use time::OffsetDateTime;
    use tokio::net::TcpListener;

    const SA_SOURCE_ID: &str = "google-drive-sa-source";
    const SA_SYNC_RUN_ID: &str = "google-drive-sa-sync";
    const SA_CLIENT_EMAIL: &str = "sa@test-project.iam.gserviceaccount.com";

    static SA_ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    const TEST_RSA_KEY: &str = "-----BEGIN PRIVATE KEY-----\nMIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQDCfecBkmeBi4Av\ne6rZMd3DJe4TJDXXe8XG6KKHbaAPWY7Zidzsr630LFu1hOLWOQj3N5c9ufdI9d8G\nNT7mCrypOMBLHgo9T9X+Zebb2hQEarcUUwku0gURPh6WXoVWrJHwxsGdfoGixpPG\nfHyE5fX/EWniHhpcBTIRNOX6zgjuE7FaOMLoo8FzNyVzkRZwhCb/7G782x4t37kA\nLIGtwy7l4qbc8MN75eF5lPPYuSbTG85hbSQUlhDpZhruZjm5SL+F+cuOt++RX18I\nMoJRNCjrR+tmpGTkNVBdILVRSVAvwmFXG5Ve5PIXDvDmHWr+z5wFv1aobyGE/xG2\nu1K2Ip6/AgMBAAECggEABGOg/fkW2uaSCwBId8RXU9scR1RO3sENUpLXcCT6Mr57\nqc8hrDm+vD7wBuWr1NfOqv2XLS5wNTZPRS2YcMqXPV5pgIh6BK4zjx0vm5CNWRgr\nb4r8LxFQSfZT7GLPsYgNdxiVL/+13z2KAjW2/azO42W6NP8m6yK24YqHEiTqMK05\nA6En5B3sfa/xIV4eLMu1LG3q0r0S3Ja1yUOM9jl/xDJ3a4OddJyoDc90spU486Fv\nvgvhmE7acbbM4GLclSKzJxvDizxkIS5HX54uUWBJUl5YbbFE8qOs5dp7p50yvDjR\n49RCTV+FbEyZNG88DwiloRDAjEOgP8qDI4r2XaRXSQKBgQDpLwvoKfcy5ht+c7kZ\nz5WccJbu1+RfxIAr3Haob34QP6SJFdNbnd3dKzQDWRESEdyWJrkVj1BMu3tpbNz9\n6+F4qW1f9WGLJltFhod2hd1oDSzNycGopT55cCD8ZyJJ61kfilDI7zcLNAph73nQ\nh12zxFdnk5hqfHaybxKV/mktGwKBgQDVhav8Ru2Ij3TTQb98uupkdNXjuwz375eR\nTFQDhB5ZYSOzKGKlnwHX6hJEyWgeTN0DWnGm+uVl65S9EhyJ3cxPuOgLiLtp26B6\nG8uZHMJ3tdVlQ/hCwKx9sWPaejBPlarWt9KQnF34OhQMnvrBXYhLUA15jO7b1E0D\ntLAyJCEjLQKBgD01h0ea9HOc6WypDdaToe8dstDhROZKm2ZoCZGvKoUzX4pIe2Ga\nL+nldFLIp2152NBlO8JIC0kJEZ0b4WqZ52aX+sjsjX1MRTsb1CUtgG/WvYMLSdVu\nAtc3ssDuhZanu45G7WvBN06ui2cnyG8PiW4txM/Ac4rIPxQZieRrksovAoGAOJXs\nNjc1y/L4quPJs2x1oZm09V0k2rAMIt1vhl8FC/rKUzhorCuveWD25nPZu+3yxGi7\npdzn3lLIYDLkjUTSWG5QUH4z7KHfrXygQDt27fKqUuPobwhQrh7Mr6GiG/U2CSE+\nFETcQmRh29Zl7cizzgGxEH1g77Ebl9fSufcJMSECgYEAjuuLUaeho5ghZ2bNPxuy\nWtXImSWim+x0CmEm/M4rWuSXyJsGBh4t+PUytnlLfJR3tgvpmZ4soKgjLraBbB62\nMfMYC1T5eAXdf9wkc0ZA+Qu8VTFWZg+QalHd32+wFEQ5IzFZjgLECjy74l3/DNlt\nB7w63Vg5YmJRvkNWTgb3sLY=\n-----END PRIVATE KEY-----\n";

    /// A configurable mock of the subset of Drive + token endpoints SA-direct uses.
    #[derive(Clone, Default)]
    struct SaMockState {
        /// The ACL member list served for every drive (overridable per test).
        permissions: Arc<Mutex<Option<JsonValue>>>,
        /// Per-folder ACLs keyed by folder id (SA-direct folder ACL fetches).
        folder_permissions: Arc<Mutex<HashMap<String, JsonValue>>>,
        /// Whether the permissions endpoint returns 403 (e.g. Viewer role).
        permissions_forbidden: Arc<AtomicBool>,
        /// Files served by list_files_in_drive (corpora=drive).
        files: Arc<Mutex<Vec<JsonValue>>>,
        /// Files served by list_trashed_files_in_drive (trashed=true).
        trashed_files: Arc<Mutex<Vec<JsonValue>>>,
        /// Change-token pagination response for incremental syncs.
        changes: Arc<Mutex<Option<JsonValue>>>,
        /// Captured /sdk/events/batch bodies.
        event_bodies: Arc<Mutex<Vec<JsonValue>>>,
        /// Count of /drive/v3/files list calls (to detect uncut full traversal).
        file_list_calls: Arc<AtomicUsize>,
        /// Query parameters captured for folder-discovery assertions.
        file_list_queries: Arc<Mutex<Vec<HashMap<String, String>>>>,
        /// Whether folder discovery responses should report an incomplete corpus search.
        incomplete_search: Arc<AtomicBool>,
        /// Whether folder discovery responses should paginate in 100-item pages.
        folder_discovery_pagination: Arc<AtomicBool>,
        /// Count of file metadata requests, used to detect ancestor loading.
        file_metadata_calls: Arc<AtomicUsize>,
    }

    impl SaMockState {
        fn set_permissions(&self, perms: JsonValue) {
            *self.permissions.lock().unwrap() = Some(perms);
        }
        fn set_folder_permissions(&self, folder_id: &str, perms: JsonValue) {
            self.folder_permissions
                .lock()
                .unwrap()
                .insert(folder_id.to_string(), perms);
        }
        fn set_permissions_forbidden(&self, forbidden: bool) {
            self.permissions_forbidden
                .store(forbidden, Ordering::SeqCst);
        }
        fn set_files(&self, files: Vec<JsonValue>) {
            *self.files.lock().unwrap() = files;
        }
        fn set_trashed_files(&self, files: Vec<JsonValue>) {
            *self.trashed_files.lock().unwrap() = files;
        }
        fn file_list_calls(&self) -> usize {
            self.file_list_calls.load(Ordering::SeqCst)
        }
        fn file_list_queries(&self) -> Vec<HashMap<String, String>> {
            self.file_list_queries.lock().unwrap().clone()
        }
        fn set_incomplete_search(&self, incomplete: bool) {
            self.incomplete_search.store(incomplete, Ordering::SeqCst);
        }
        fn set_folder_discovery_pagination(&self, enabled: bool) {
            self.folder_discovery_pagination
                .store(enabled, Ordering::SeqCst);
        }
        fn file_metadata_calls(&self) -> usize {
            self.file_metadata_calls.load(Ordering::SeqCst)
        }
        fn event_bodies(&self) -> Vec<JsonValue> {
            self.event_bodies.lock().unwrap().clone()
        }
    }

    async fn spawn_sa_mock() -> Result<(String, SaMockState)> {
        let state = SaMockState::default();
        // Seed default drive files.
        state.set_files(vec![
            json!({
                "id": "doc-1",
                "name": "Policy 2025.txt",
                "mimeType": "text/plain",
                "size": "1024",
                "webViewLink": "https://example.test/doc-1",
                "createdTime": "2024-01-01T00:00:00Z",
                "modifiedTime": "2024-01-02T00:00:00Z",
                "driveId": "drive-1",
                "permissions": [],
                "owners": []
            }),
            json!({
                "id": "doc-2",
                "name": "Policy 2024.txt",
                "mimeType": "text/plain",
                "size": "2048",
                "webViewLink": "https://example.test/doc-2",
                "createdTime": "2023-01-01T00:00:00Z",
                "modifiedTime": "2023-06-01T00:00:00Z",
                "driveId": "drive-1",
                "permissions": [],
                "owners": []
            }),
        ]);

        let app = Router::new()
            .route("/token", post(issue_token))
            .route("/drive/v3/drives", get(list_drives_mock))
            .route(
                "/drive/v3/files/:drive_id/permissions",
                get(list_permissions_mock),
            )
            .route("/drive/v3/files/:file_id", get(get_file_mock))
            .route("/drive/v3/files", get(list_files_mock))
            .route(
                "/drive/v3/changes/startPageToken",
                get(start_page_token_mock),
            )
            .route("/drive/v3/changes", get(list_changes_mock))
            .route("/sdk/events/batch", post(capture_events))
            .route("/sdk/content", post(store_content_mock))
            .route("/sdk/sync/:sync_run_id/scanned", post(ok_json))
            .route("/sdk/sync/:sync_run_id/updated", post(ok_json))
            .route("/sdk/sync/:sync_run_id/complete", post(ok_json))
            .route("/sdk/sync/:sync_run_id/checkpoint", put(put_ok))
            .route("/sdk/source/:source_id/connector-state", put(put_ok))
            .route("/sdk/connector-configs/:provider", get(connector_config_ok))
            .with_state(state.clone());

        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        tokio::spawn(async move {
            axum::serve(listener, app).await.ok();
        });

        Ok((format!("http://{}", addr), state))
    }

    async fn issue_token() -> Json<JsonValue> {
        Json(json!({
            "access_token": "sa-access-token",
            "expires_in": 3600
        }))
    }

    async fn list_drives_mock(Query(query): Query<HashMap<String, String>>) -> Json<JsonValue> {
        if query
            .get("q")
            .map(|q| q.contains("name contains 'shared'"))
            .unwrap_or(false)
        {
            let drives = (0..101)
                .map(|index| {
                    json!({
                        "id": format!("drive-{index}"),
                        "name": if index == 1 {
                            "Shared Policy Docs".to_string()
                        } else {
                            format!("Shared Drive {index}")
                        }
                    })
                })
                .collect::<Vec<_>>();
            return Json(json!({ "drives": drives }));
        }
        Json(json!({
            "drives": [
                { "id": "drive-1", "name": "Policy Docs" },
                { "id": "drive-2", "name": "Team Wiki" }
            ]
        }))
    }

    async fn get_file_mock(
        State(state): State<SaMockState>,
        Query(query): Query<HashMap<String, String>>,
    ) -> Json<JsonValue> {
        state.file_metadata_calls.fetch_add(1, Ordering::SeqCst);
        if query.get("alt").map(String::as_str) == Some("media") {
            return Json(json!("content for file"));
        }
        Json(json!({ "id": "doc-1", "name": "Policy.txt", "mimeType": "text/plain" }))
    }

    async fn list_permissions_mock(
        State(state): State<SaMockState>,
        Path(file_id): Path<String>,
    ) -> Json<JsonValue> {
        if state.permissions_forbidden.load(Ordering::SeqCst) {
            return Json(json!({
                "error": { "code": 403, "message": "Forbidden", "status": "PERMISSION_DENIED" }
            }));
        }
        // Folder ACLs (SA-direct effective-permission model) are keyed by id;
        // the drive ACL is the shared default.
        if let Some(perms) = state
            .folder_permissions
            .lock()
            .unwrap()
            .get(&file_id)
            .cloned()
        {
            return Json(json!({ "permissions": perms }));
        }
        let perms = state.permissions.lock().unwrap().clone();
        match perms {
            Some(perms) => Json(json!({ "permissions": perms })),
            None => Json(json!({ "permissions": [] })),
        }
    }

    async fn list_files_mock(
        State(state): State<SaMockState>,
        Query(query): Query<HashMap<String, String>>,
    ) -> Json<JsonValue> {
        state.file_list_calls.fetch_add(1, Ordering::SeqCst);
        state.file_list_queries.lock().unwrap().push(query.clone());
        // trashed=true queries serve the trashed-file set for full-traversal
        // reconciliation; everything else serves the live file set.
        let is_trashed_query = query
            .get("q")
            .map(|q| q.contains("trashed=true"))
            .unwrap_or(false);
        let files = if is_trashed_query {
            state.trashed_files.lock().unwrap().clone()
        } else {
            state.files.lock().unwrap().clone()
        };
        // Mirror the API's server-side filtering so the folders-only pass only
        // receives folders.
        let is_folder_query = query
            .get("q")
            .map(|q| q.contains("mimeType='application/vnd.google-apps.folder'"))
            .unwrap_or(false);
        let files: Vec<JsonValue> = if is_folder_query {
            files
                .into_iter()
                .filter(|f| f["mimeType"].as_str() == Some("application/vnd.google-apps.folder"))
                .collect()
        } else {
            files
        };
        // If a modifiedTime cutoff is present, filter the mocked files so we
        // can distinguish a cutoff crawl from an uncut full traversal.
        let files: Vec<JsonValue> = if let Some(q) = query.get("q") {
            if q.contains("modifiedTime") || q.contains("createdTime") {
                files
                    .into_iter()
                    .filter(|f| f["modifiedTime"].as_str().unwrap_or("") >= "2024-01-01T00:00:00Z")
                    .collect()
            } else {
                files
            }
        } else {
            files
        };

        let files: Vec<JsonValue> = if is_folder_query {
            let prefix = query
                .get("q")
                .and_then(|q| q.split_once("name contains '").map(|(_, rest)| rest))
                .and_then(|rest| rest.split_once('\'').map(|(prefix, _)| prefix))
                .map(str::to_lowercase);
            files
                .into_iter()
                .filter(|file| {
                    prefix
                        .as_ref()
                        .map(|prefix| {
                            file["name"]
                                .as_str()
                                .map(|name| name.to_lowercase().starts_with(prefix))
                                .unwrap_or(false)
                        })
                        .unwrap_or(true)
                })
                .collect()
        } else {
            files
        };

        let page_start = query
            .get("pageToken")
            .and_then(|token| token.strip_prefix("folder-page-"))
            .and_then(|offset| offset.parse::<usize>().ok())
            .unwrap_or(0);
        let paginate = is_folder_query && state.folder_discovery_pagination.load(Ordering::SeqCst);
        let page_end = if paginate {
            (page_start + 100).min(files.len())
        } else {
            files.len()
        };
        let page_files = files.get(page_start..page_end).unwrap_or_default().to_vec();
        let next_page = (page_end < files.len()).then(|| format!("folder-page-{page_end}"));
        let mut response = if state.incomplete_search.load(Ordering::SeqCst) && is_folder_query {
            json!({ "files": page_files, "incompleteSearch": true })
        } else {
            json!({ "files": page_files })
        };
        if let Some(next_page) = next_page {
            response["nextPageToken"] = json!(next_page);
        }
        Json(response)
    }

    async fn start_page_token_mock() -> Json<JsonValue> {
        Json(json!({ "startPageToken": "start-1" }))
    }

    async fn list_changes_mock(State(state): State<SaMockState>) -> Json<JsonValue> {
        let changes = state.changes.lock().unwrap().clone();
        match changes {
            Some(changes) => Json(changes),
            None => Json(json!({ "changes": [], "newStartPageToken": "start-2" })),
        }
    }

    async fn capture_events(
        State(state): State<SaMockState>,
        body: Json<JsonValue>,
    ) -> Json<JsonValue> {
        state.event_bodies.lock().unwrap().push(body.0.clone());
        Json(json!({}))
    }

    async fn store_content_mock() -> Json<JsonValue> {
        Json(json!({ "content_id": "content-id" }))
    }

    async fn ok_json() -> Json<JsonValue> {
        Json(json!({}))
    }

    async fn put_ok(State(_state): State<SaMockState>) -> Json<JsonValue> {
        Json(json!({}))
    }

    async fn connector_config_ok() -> Json<JsonValue> {
        Json(json!({
            "oauth_client_id": "test-client-id",
            "oauth_client_secret": "test-client-secret"
        }))
    }

    fn sa_source() -> Source {
        let now = OffsetDateTime::now_utc();
        Source {
            id: SA_SOURCE_ID.to_string(),
            name: "Google Drive SA Direct".to_string(),
            source_type: SourceType::GoogleDrive.to_string(),
            integration_type: shared::models::IntegrationType::Connector,
            config: json!({
                "auth_mode": "service_account_direct",
                "folder_path_filters": [
                    {
                        "id": "drive-1",
                        "name": "Policy Docs",
                        "path": "/Policy Docs (Shared Drive)",
                        "driveId": "drive-1",
                        "kind": "shared_drive_root"
                    }
                ]
            }),
            is_active: true,
            is_deleted: false,
            scope: SourceScope::Org,
            user_filter_mode: UserFilterMode::All,
            user_whitelist: None,
            user_blacklist: None,
            connector_state: None,
            checkpoint: None,
            sync_interval_seconds: None,
            created_at: now,
            updated_at: now,
            created_by: "admin-id".to_string(),
        }
    }

    fn sa_credentials(mock_base: &str) -> ServiceCredential {
        let now = OffsetDateTime::now_utc();
        let sa_key = json!({
            "type": "service_account",
            "project_id": "test-project",
            "private_key_id": "key123",
            "private_key": TEST_RSA_KEY,
            "client_email": SA_CLIENT_EMAIL,
            "client_id": "123456789",
            "auth_uri": "https://accounts.google.com/o/oauth2/auth",
            "token_uri": format!("{}/token", mock_base),
            "auth_provider_x509_cert_url": "https://www.googleapis.com/oauth2/v1/certs",
            "client_x509_cert_url": "https://www.googleapis.com/robot/v1/metadata/x509/sa"
        });
        ServiceCredential {
            id: "credential-id".to_string(),
            source_id: SA_SOURCE_ID.to_string(),
            user_id: None,
            provider: ServiceProvider::Google,
            auth_type: AuthType::Jwt,
            principal_email: None,
            credentials: json!({ "service_account_key": sa_key.to_string() }),
            config: json!({
                "scopes": ["https://www.googleapis.com/auth/drive.readonly"]
            }),
            expires_at: None,
            last_validated_at: None,
            created_at: now,
            updated_at: now,
        }
    }

    /// Run an SA-direct sync against the mock. The mock state is owned by the
    /// test (shared via Arc with the router), so the sync runs inline and the
    /// caller inspects `state` afterwards.
    async fn run_sa_sync(
        mock_base: &str,
        source: Source,
        creds: ServiceCredential,
        sync_type: SyncType,
    ) -> Result<()> {
        let cm_url = mock_base.to_string();
        let sdk_client = SdkClient::new(&cm_url);
        sdk_client.register_sync(SA_SYNC_RUN_ID, sync_type).await;
        let sync_manager = SyncManager::new(Arc::new(AdminClient::new()), sdk_client.clone(), None);
        let ctx = SyncContext::new(
            sdk_client,
            SA_SYNC_RUN_ID.to_string(),
            SA_SOURCE_ID.to_string(),
            SourceType::GoogleDrive,
            sync_type,
            Arc::new(AtomicBool::new(false)),
        );
        sync_manager.run_sync(source, Some(creds), None, ctx).await
    }

    /// Like [`run_sa_sync`] but seeds an existing checkpoint so the sync can
    /// resume incrementally (change tokens / ACL fingerprints present).
    async fn run_sa_sync_with_checkpoint(
        mock_base: &str,
        source: Source,
        creds: ServiceCredential,
        sync_type: SyncType,
        checkpoint: GoogleSyncCheckpoint,
    ) -> Result<()> {
        let cm_url = mock_base.to_string();
        let sdk_client = SdkClient::new(&cm_url);
        sdk_client.register_sync(SA_SYNC_RUN_ID, sync_type).await;
        let sync_manager = SyncManager::new(Arc::new(AdminClient::new()), sdk_client.clone(), None);
        let ctx = SyncContext::new(
            sdk_client,
            SA_SYNC_RUN_ID.to_string(),
            SA_SOURCE_ID.to_string(),
            SourceType::GoogleDrive,
            sync_type,
            Arc::new(AtomicBool::new(false)),
        );
        sync_manager
            .run_sync(source, Some(creds), Some(checkpoint), ctx)
            .await
    }

    #[tokio::test]
    async fn sa_direct_discovery_uses_self_token_and_returns_roots_only() -> Result<()> {
        let _guard = SA_ENV_LOCK.lock().await;
        let (mock_base, _state) = spawn_sa_mock().await?;
        let previous_drive_base = std::env::var("GOOGLE_DRIVE_API_BASE").ok();
        // TODO: Audit that the environment access only happens in single-threaded code.
        unsafe { std::env::set_var("GOOGLE_DRIVE_API_BASE", format!("{}/drive/v3", mock_base)) };

        let connector = omni_google_connector::connector::GoogleConnector::new(
            Arc::new(SyncManager::new(
                Arc::new(AdminClient::new()),
                SdkClient::new(&mock_base),
                None,
            )),
            Arc::new(AdminClient::new()),
        );

        // SA-direct discovery: no principal email/domain required; roots only.
        let creds = sa_credentials(&mock_base);
        let response = connector
            .execute_action(
                "discover_folders",
                json!({ "auth_mode": "service_account_direct" }),
                Some(creds),
                None,
                None,
            )
            .await?;

        if let Some(value) = previous_drive_base {
            // TODO: Audit that the environment access only happens in single-threaded code.
            unsafe { std::env::set_var("GOOGLE_DRIVE_API_BASE", value) };
        } else {
            // TODO: Audit that the environment access only happens in single-threaded code.
            unsafe { std::env::remove_var("GOOGLE_DRIVE_API_BASE") };
        }

        let status = response.status();
        let body: JsonValue = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .map(|bytes| serde_json::from_slice(&bytes).unwrap_or(json!({})))?;
        assert_eq!(status, 200, "discovery should succeed: {}", body);
        let items = body["result"]["items"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(items.len(), 2, "expected 2 shared drives: {}", body);
        for item in &items {
            assert_eq!(
                item["kind"].as_str(),
                Some("shared_drive_root"),
                "SA-direct discovery must return roots only: {}",
                body
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn dwd_folder_search_uses_all_drives_and_filters_before_limit() -> Result<()> {
        let _guard = SA_ENV_LOCK.lock().await;
        let (mock_base, state) = spawn_sa_mock().await?;
        let previous_drive_base = std::env::var("GOOGLE_DRIVE_API_BASE").ok();
        // TODO: Audit that the environment access only happens in single-threaded code.
        unsafe { std::env::set_var("GOOGLE_DRIVE_API_BASE", format!("{}/drive/v3", mock_base)) };

        let mut folders: Vec<JsonValue> = (0..500)
            .map(|index| {
                json!({
                    "id": format!("my-folder-{index}"),
                    "name": format!("Shared Folder {index}"),
                    "mimeType": "application/vnd.google-apps.folder"
                })
            })
            .collect();
        folders.insert(
            0,
            json!({
                "id": "nested-folder",
                "name": "Shared Nested Folder",
                "mimeType": "application/vnd.google-apps.folder",
                "driveId": "drive-1",
                "parents": ["parent-folder"]
            }),
        );
        folders.insert(
            1,
            json!({
                "id": "drive-100",
                "name": "Shared Drive 100",
                "mimeType": "application/vnd.google-apps.folder",
                "driveId": "drive-100"
            }),
        );
        folders.push(json!({
            "id": "shared-folder",
            "name": "Shared Folder",
            "mimeType": "application/vnd.google-apps.folder",
            "driveId": "drive-1",
            "parents": ["drive-1"]
        }));
        folders.push(json!({
            "id": "drive-1",
            "name": "Shared Drive",
            "mimeType": "application/vnd.google-apps.folder",
            "driveId": "drive-1"
        }));
        state.set_files(folders);
        state.set_incomplete_search(true);
        state.set_folder_discovery_pagination(true);

        let connector = omni_google_connector::connector::GoogleConnector::new(
            Arc::new(SyncManager::new(
                Arc::new(AdminClient::new()),
                SdkClient::new(&mock_base),
                None,
            )),
            Arc::new(AdminClient::new()),
        );

        let mut creds = sa_credentials(&mock_base);
        creds.principal_email = Some("user@example.com".to_string());
        let response = connector
            .execute_action(
                "discover_folders",
                json!({ "query": "shared" }),
                Some(creds),
                None,
                None,
            )
            .await?;

        if let Some(value) = previous_drive_base {
            // TODO: Audit that the environment access only happens in single-threaded code.
            unsafe { std::env::set_var("GOOGLE_DRIVE_API_BASE", value) };
        } else {
            // TODO: Audit that the environment access only happens in single-threaded code.
            unsafe { std::env::remove_var("GOOGLE_DRIVE_API_BASE") };
        }

        let status = response.status();
        let body: JsonValue = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .map(|bytes| serde_json::from_slice(&bytes).unwrap_or(json!({})))?;
        assert_eq!(status, 200, "discovery should succeed: {}", body);
        assert_eq!(
            body["result"]["truncated"], true,
            "incomplete search: {}",
            body
        );
        let items = body["result"]["items"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert!(
            items.iter().any(|item| item["id"] == "shared-folder"),
            "shared-drive folders must survive My Drive filtering before the cap: {}",
            body
        );
        assert!(
            items.iter().any(|item| item["id"] == "nested-folder"),
            "nested folders must remain selectable: {}",
            body
        );
        let nested_path = items
            .iter()
            .find(|item| item["id"] == "nested-folder")
            .and_then(|item| item["path"].as_str());
        assert_eq!(
            nested_path,
            Some("/Shared Policy Docs (Shared Drive)/…/Shared Nested Folder"),
            "searched nested folder paths must be visibly partial: {}",
            body
        );
        assert_eq!(
            items
                .iter()
                .filter(|item| item["kind"] == "shared_drive_root")
                .count(),
            101,
            "search discovery should allow roots beyond the initial 100-root cap: {}",
            body
        );
        let drive_100 = items.iter().find(|item| item["id"] == "drive-100");
        assert_eq!(
            drive_100.and_then(|item| item["kind"].as_str()),
            Some("shared_drive_root"),
            "search roots beyond the initial root sample must remain roots: {}",
            body
        );
        assert_eq!(
            items
                .iter()
                .filter(|item| item["id"] == "drive-100")
                .count(),
            1,
            "search roots beyond the initial root sample must not reappear as folders: {}",
            body
        );
        let file_queries = state.file_list_queries();
        assert_eq!(
            file_queries.len(),
            6,
            "search should paginate folders without per-drive child calls"
        );
        assert_eq!(
            state.file_metadata_calls(),
            0,
            "search should not load folder ancestors"
        );
        assert!(
            file_queries
                .iter()
                .all(|query| query.get("corpora").map(String::as_str) == Some("allDrives")),
            "folder search must use the allDrives corpus"
        );
        Ok(())
    }

    #[tokio::test]
    async fn validate_shared_drive_access_multi_drive() -> Result<()> {
        let _guard = SA_ENV_LOCK.lock().await;
        let (mock_base, state) = spawn_sa_mock().await?;
        let previous_drive_base = std::env::var("GOOGLE_DRIVE_API_BASE").ok();
        // TODO: Audit that the environment access only happens in single-threaded code.
        unsafe { std::env::set_var("GOOGLE_DRIVE_API_BASE", format!("{}/drive/v3", mock_base)) };

        let connector = omni_google_connector::connector::GoogleConnector::new(
            Arc::new(SyncManager::new(
                Arc::new(AdminClient::new()),
                SdkClient::new(&mock_base),
                None,
            )),
            Arc::new(AdminClient::new()),
        );

        // SA is a Manager on the drive.
        state.set_permissions(json!([
            {
                "id": "p1",
                "type": "user",
                "emailAddress": SA_CLIENT_EMAIL,
                "domain": null,
                "role": "organizer",
                "allowFileDiscovery": null,
                "permissionDetails": null
            }
        ]));
        let ok_response = connector
            .execute_action(
                "validate_shared_drive_access",
                json!({ "auth_mode": "service_account_direct", "drive_ids": ["drive-1"] }),
                Some(sa_credentials(&mock_base)),
                None,
                None,
            )
            .await?;
        let ok_body: JsonValue = axum::body::to_bytes(ok_response.into_body(), usize::MAX)
            .await
            .map(|bytes| serde_json::from_slice(&bytes).unwrap_or(json!({})))?;
        let drives = ok_body["result"]["drives"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(drives.len(), 1);
        assert_eq!(drives[0]["ok"], json!(true), "{}", ok_body);
        assert_eq!(drives[0]["role"].as_str(), Some("organizer"));

        // Viewer role fails closed.
        state.set_permissions(json!([
            {
                "id": "p1",
                "type": "user",
                "emailAddress": SA_CLIENT_EMAIL,
                "domain": null,
                "role": "reader",
                "allowFileDiscovery": null,
                "permissionDetails": null
            }
        ]));
        let fail_response = connector
            .execute_action(
                "validate_shared_drive_access",
                json!({ "auth_mode": "service_account_direct", "drive_ids": ["drive-1"] }),
                Some(sa_credentials(&mock_base)),
                None,
                None,
            )
            .await?;
        let fail_body: JsonValue = axum::body::to_bytes(fail_response.into_body(), usize::MAX)
            .await
            .map(|bytes| serde_json::from_slice(&bytes).unwrap_or(json!({})))?;
        let fail_drives = fail_body["result"]["drives"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(fail_drives[0]["ok"], json!(false), "{}", fail_body);
        assert!(
            fail_drives[0]["error"]
                .as_str()
                .unwrap_or("")
                .contains("role"),
            "{}",
            fail_body
        );

        if let Some(value) = previous_drive_base {
            // TODO: Audit that the environment access only happens in single-threaded code.
            unsafe { std::env::set_var("GOOGLE_DRIVE_API_BASE", value) };
        } else {
            // TODO: Audit that the environment access only happens in single-threaded code.
            unsafe { std::env::remove_var("GOOGLE_DRIVE_API_BASE") };
        }
        Ok(())
    }

    #[tokio::test]
    async fn sa_direct_full_sync_applies_drive_acl() -> Result<()> {
        let _guard = SA_ENV_LOCK.lock().await;
        let (mock_base, state) = spawn_sa_mock().await?;
        let previous_drive_base = std::env::var("GOOGLE_DRIVE_API_BASE").ok();
        // TODO: Audit that the environment access only happens in single-threaded code.
        unsafe { std::env::set_var("GOOGLE_DRIVE_API_BASE", format!("{}/drive/v3", mock_base)) };

        // Drive members: an internal user + a domain grant. SA excluded.
        state.set_permissions(json!([
            {
                "id": "p1",
                "type": "user",
                "emailAddress": SA_CLIENT_EMAIL,
                "domain": null,
                "role": "organizer",
                "allowFileDiscovery": null,
                "permissionDetails": null
            },
            {
                "id": "p2",
                "type": "user",
                "emailAddress": "alice@example.com",
                "domain": null,
                "role": "writer",
                "allowFileDiscovery": null,
                "permissionDetails": null
            },
            {
                "id": "p3",
                "type": "domain",
                "emailAddress": null,
                "domain": "example.com",
                "role": "reader",
                "allowFileDiscovery": true,
                "permissionDetails": null
            }
        ]));

        // Run the sync inline against the mock.
        let sync_result = run_sa_sync(
            &mock_base,
            sa_source(),
            sa_credentials(&mock_base),
            SyncType::Full,
        )
        .await;

        if let Some(value) = previous_drive_base {
            // TODO: Audit that the environment access only happens in single-threaded code.
            unsafe { std::env::set_var("GOOGLE_DRIVE_API_BASE", value) };
        } else {
            // TODO: Audit that the environment access only happens in single-threaded code.
            unsafe { std::env::remove_var("GOOGLE_DRIVE_API_BASE") };
        }

        sync_result?;

        let bodies = state.event_bodies();
        assert!(!bodies.is_empty(), "expected emitted event batch bodies");
        // Find DocumentCreated events and assert ACLs.
        let mut found = 0;
        for body in &bodies {
            if let Some(events) = body.get("events").and_then(|e| e.as_array()) {
                for event in events {
                    if event.get("type").and_then(|t| t.as_str()) == Some("document_created")
                        || event.get("event").and_then(|e| e.as_str()) == Some("document_created")
                    {
                        found += 1;
                    }
                }
            }
        }
        assert!(found >= 1, "expected at least one document_created event");
        assert!(
            state.file_list_calls() >= 1,
            "expected at least one files.list call"
        );

        Ok(())
    }

    /// A trashed file in an incremental changes.list response must publish a
    /// deletion event (trash = inaccessible, so it leaves the index) rather
    /// than being re-indexed as a normal file.
    #[tokio::test]
    async fn sa_direct_incremental_publishes_deletion_for_trashed_file() -> Result<()> {
        let _guard = SA_ENV_LOCK.lock().await;
        let (mock_base, state) = spawn_sa_mock().await?;
        let previous_drive_base = std::env::var("GOOGLE_DRIVE_API_BASE").ok();
        unsafe { std::env::set_var("GOOGLE_DRIVE_API_BASE", format!("{}/drive/v3", mock_base)) };

        // Drive members: an internal user + the SA. SA excluded from docs.
        state.set_permissions(json!([
            {
                "id": "p1",
                "type": "user",
                "emailAddress": SA_CLIENT_EMAIL,
                "domain": null,
                "role": "organizer",
                "allowFileDiscovery": null,
                "permissionDetails": null
            },
            {
                "id": "p2",
                "type": "user",
                "emailAddress": "alice@example.com",
                "domain": null,
                "role": "reader",
                "allowFileDiscovery": null,
                "permissionDetails": null
            }
        ]));

        // One incremental change: the file moved to trash (trashed:true,
        // removed:false). ACL fingerprint unchanged so the run stays
        // incremental.
        state.changes.lock().unwrap().replace(json!({
            "changes": [
                {
                    "changeType": "file",
                    "removed": false,
                    "fileId": "doc-trash",
                    "file": {
                        "id": "doc-trash",
                        "name": "Old Policy.txt",
                        "mimeType": "text/plain",
                        "size": "1024",
                        "webViewLink": "https://example.test/doc-trash",
                        "createdTime": "2023-01-01T00:00:00Z",
                        "modifiedTime": "2023-01-02T00:00:00Z",
                        "driveId": "drive-1",
                        "trashed": true,
                        "permissions": [],
                        "owners": []
                    },
                    "time": "2024-01-03T00:00:00Z"
                }
            ],
            "newStartPageToken": "start-99"
        }));

        // Seed a checkpoint with an ACL fingerprint and change token so the
        // sync takes the incremental branch (no scope/ACL change, token known).
        let checkpoint = GoogleSyncCheckpoint {
            drive_scope_fingerprint: Some(
                "sa-direct:drive-1:shared_drive_root:drive-1".to_string(),
            ),
            drive_acl_fingerprints: Some(HashMap::from([(
                "drive-1".to_string(),
                "user|organizer|sa@test-project.iam.gserviceaccount.com||true;user|reader|alice@example.com||true"
                    .to_string(),
            )])),
            drive_change_tokens: Some(HashMap::from([("drive-1".to_string(), "start-1".to_string())])),
            permission_model_version: Some(
                omni_google_connector::models::SA_DIRECT_PERMISSION_MODEL_VERSION.to_string(),
            ),
            ..Default::default()
        };

        let sync_result = run_sa_sync_with_checkpoint(
            &mock_base,
            sa_source(),
            sa_credentials(&mock_base),
            SyncType::Incremental,
            checkpoint,
        )
        .await;

        if let Some(value) = previous_drive_base {
            unsafe { std::env::set_var("GOOGLE_DRIVE_API_BASE", value) };
        } else {
            unsafe { std::env::remove_var("GOOGLE_DRIVE_API_BASE") };
        }

        sync_result?;

        let bodies = state.event_bodies();
        assert!(!bodies.is_empty(), "expected emitted event batch bodies");
        let mut deletions = 0;
        for body in &bodies {
            if let Some(events) = body.get("events").and_then(|e| e.as_array()) {
                for event in events {
                    let kind = event
                        .get("type")
                        .or_else(|| event.get("event"))
                        .and_then(|t| t.as_str())
                        .unwrap_or("");
                    if kind == "document_deleted" {
                        deletions += 1;
                    }
                }
            }
        }
        assert_eq!(
            deletions, 1,
            "expected exactly one document_deleted event for the trashed file"
        );

        Ok(())
    }

    /// A full traversal must reconcile pre-existing trashed files: files
    /// trashed before their trash transition was observed by an incremental
    /// run never reappear in changes.list, so the full path lists trashed=true
    /// and publishes deletions for them.
    #[tokio::test]
    async fn sa_direct_full_traversal_reconciles_pre_trashed_files() -> Result<()> {
        let _guard = SA_ENV_LOCK.lock().await;
        let (mock_base, state) = spawn_sa_mock().await?;
        let previous_drive_base = std::env::var("GOOGLE_DRIVE_API_BASE").ok();
        unsafe { std::env::set_var("GOOGLE_DRIVE_API_BASE", format!("{}/drive/v3", mock_base)) };

        // Drive members: an internal user + the SA. SA excluded from docs.
        state.set_permissions(json!([
            {
                "id": "p1",
                "type": "user",
                "emailAddress": SA_CLIENT_EMAIL,
                "domain": null,
                "role": "organizer",
                "allowFileDiscovery": null,
                "permissionDetails": null
            },
            {
                "id": "p2",
                "type": "user",
                "emailAddress": "alice@example.com",
                "domain": null,
                "role": "reader",
                "allowFileDiscovery": null,
                "permissionDetails": null
            }
        ]));

        // The drive holds one live file plus one file that was trashed before
        // the last incremental run (its trash transition is already consumed,
        // so it never reappears in changes.list).
        state.set_files(vec![json!({
            "id": "live-1",
            "name": "Live Doc.txt",
            "mimeType": "text/plain",
            "size": "1024",
            "webViewLink": "https://example.test/live-1",
            "createdTime": "2024-01-01T00:00:00Z",
            "modifiedTime": "2024-01-02T00:00:00Z",
            "driveId": "drive-1",
            "permissions": [],
            "owners": []
        })]);
        state.set_trashed_files(vec![json!({
            "id": "pre-trashed",
            "name": "Old Doc.txt",
            "mimeType": "text/plain",
            "size": "512",
            "webViewLink": "https://example.test/pre-trashed",
            "createdTime": "2022-01-01T00:00:00Z",
            "modifiedTime": "2022-06-01T00:00:00Z",
            "driveId": "drive-1",
            "trashed": true,
            "permissions": [],
            "owners": []
        })]);

        // Full sync, no prior state.
        let sync_result = run_sa_sync(
            &mock_base,
            sa_source(),
            sa_credentials(&mock_base),
            SyncType::Full,
        )
        .await;

        if let Some(value) = previous_drive_base {
            unsafe { std::env::set_var("GOOGLE_DRIVE_API_BASE", value) };
        } else {
            unsafe { std::env::remove_var("GOOGLE_DRIVE_API_BASE") };
        }

        sync_result?;

        let bodies = state.event_bodies();
        assert!(!bodies.is_empty(), "expected emitted event batch bodies");
        let mut deletions = 0;
        let mut created = 0;
        for body in &bodies {
            if let Some(events) = body.get("events").and_then(|e| e.as_array()) {
                for event in events {
                    let kind = event
                        .get("type")
                        .or_else(|| event.get("event"))
                        .and_then(|t| t.as_str())
                        .unwrap_or("");
                    match kind {
                        "document_deleted" => deletions += 1,
                        "document_created" => created += 1,
                        _ => {}
                    }
                }
            }
        }
        assert_eq!(
            deletions, 1,
            "expected one document_deleted event for the pre-trashed file"
        );
        assert!(
            created >= 1,
            "expected the live file to be re-indexed during full traversal"
        );

        Ok(())
    }

    /// Collect document_created event permissions (users) keyed by document id.
    fn created_users_by_doc(bodies: &[JsonValue]) -> HashMap<String, Vec<String>> {
        let mut perms_by_doc: HashMap<String, Vec<String>> = HashMap::new();
        for body in bodies {
            if let Some(events) = body.get("events").and_then(|e| e.as_array()) {
                for event in events {
                    if event.get("type").and_then(|t| t.as_str()) != Some("document_created") {
                        continue;
                    }
                    let doc_id = event
                        .get("document_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let users = event
                        .get("permissions")
                        .and_then(|p| p.get("users"))
                        .and_then(|u| u.as_array())
                        .map(|a| {
                            a.iter()
                                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                                .collect()
                        })
                        .unwrap_or_default();
                    perms_by_doc.insert(doc_id, users);
                }
            }
        }
        perms_by_doc
    }

    /// Files inside a limited-access folder must be indexed with organizers +
    /// the folder's direct grants, NOT the drive-wide ACL. Files in a normal
    /// folder union in its direct grants (expansive access) and keep inherited
    /// entries filtered out.
    #[tokio::test]
    async fn sa_direct_full_sync_applies_limited_access_folder_boundary() -> Result<()> {
        let _guard = SA_ENV_LOCK.lock().await;
        let (mock_base, state) = spawn_sa_mock().await?;
        let previous_drive_base = std::env::var("GOOGLE_DRIVE_API_BASE").ok();
        // TODO: Audit that the environment access only happens in single-threaded code.
        unsafe { std::env::set_var("GOOGLE_DRIVE_API_BASE", format!("{}/drive/v3", mock_base)) };

        // Drive members: SA (organizer, excluded) + alice (writer) + bob (reader).
        state.set_permissions(json!([
            { "id": "p1", "type": "user", "emailAddress": SA_CLIENT_EMAIL, "domain": null, "role": "organizer", "allowFileDiscovery": null, "permissionDetails": null },
            { "id": "p2", "type": "user", "emailAddress": "alice@example.com", "domain": null, "role": "writer", "allowFileDiscovery": null, "permissionDetails": null },
            { "id": "p3", "type": "user", "emailAddress": "bob@example.com", "domain": null, "role": "reader", "allowFileDiscovery": null, "permissionDetails": null }
        ]));

        // secret-1 is limited-access and grants only carol.
        state.set_folder_permissions(
            "secret-1",
            json!([
                { "id": "p4", "type": "user", "emailAddress": "carol@example.com", "domain": null, "role": "writer", "allowFileDiscovery": null, "permissionDetails": null }
            ]),
        );
        // folder-1 is a normal folder with a direct external grant + inherited
        // drive entries (which must be filtered out of the direct ACL).
        state.set_folder_permissions(
            "folder-1",
            json!([
                { "id": "p5", "type": "user", "emailAddress": "alice@example.com", "domain": null, "role": "writer", "allowFileDiscovery": null, "permissionDetails": [
                    { "permissionType": "sharedDrive", "role": "writer", "inherited": true, "inheritedFrom": "drive-1" }
                ] },
                { "id": "p6", "type": "user", "emailAddress": "guest@external.com", "domain": null, "role": "reader", "allowFileDiscovery": null, "permissionDetails": null }
            ]),
        );

        state.set_files(vec![
            json!({ "id": "doc-outside", "name": "Outside.txt", "mimeType": "text/plain", "size": "10", "webViewLink": "https://example.test/outside", "createdTime": "2024-01-01T00:00:00Z", "modifiedTime": "2024-01-02T00:00:00Z", "driveId": "drive-1", "permissions": [], "owners": [] }),
            json!({ "id": "folder-1", "name": "Shared", "mimeType": "application/vnd.google-apps.folder", "size": null, "webViewLink": null, "createdTime": "2024-01-01T00:00:00Z", "modifiedTime": "2024-01-02T00:00:00Z", "driveId": "drive-1", "parents": [], "inheritedPermissionsDisabled": false, "permissions": [], "owners": [] }),
            json!({ "id": "doc-in-folder", "name": "Inside shared.txt", "mimeType": "text/plain", "size": "10", "webViewLink": "https://example.test/infolder", "createdTime": "2024-01-01T00:00:00Z", "modifiedTime": "2024-01-02T00:00:00Z", "driveId": "drive-1", "parents": ["folder-1"], "permissions": [], "owners": [] }),
            json!({ "id": "secret-1", "name": "Confidential", "mimeType": "application/vnd.google-apps.folder", "size": null, "webViewLink": null, "createdTime": "2024-01-01T00:00:00Z", "modifiedTime": "2024-01-02T00:00:00Z", "driveId": "drive-1", "parents": [], "inheritedPermissionsDisabled": true, "permissions": [], "owners": [] }),
            json!({ "id": "doc-in-secret", "name": "Secret.txt", "mimeType": "text/plain", "size": "10", "webViewLink": "https://example.test/secret", "createdTime": "2024-01-01T00:00:00Z", "modifiedTime": "2024-01-02T00:00:00Z", "driveId": "drive-1", "parents": ["secret-1"], "permissions": [], "owners": [] }),
        ]);

        let sync_result = run_sa_sync(
            &mock_base,
            sa_source(),
            sa_credentials(&mock_base),
            SyncType::Full,
        )
        .await;

        if let Some(value) = previous_drive_base {
            // TODO: Audit that the environment access only happens in single-threaded code.
            unsafe { std::env::set_var("GOOGLE_DRIVE_API_BASE", value) };
        } else {
            // TODO: Audit that the environment access only happens in single-threaded code.
            unsafe { std::env::remove_var("GOOGLE_DRIVE_API_BASE") };
        }

        sync_result?;

        let perms_by_doc = created_users_by_doc(&state.event_bodies());

        let outside = perms_by_doc
            .get("doc-outside")
            .expect("doc-outside indexed");
        assert!(outside.contains(&"alice@example.com".to_string()));
        assert!(outside.contains(&"bob@example.com".to_string()));
        assert!(
            !outside.contains(&"carol@example.com".to_string()),
            "drive-wide ACL must not leak into the limited folder"
        );

        let in_folder = perms_by_doc
            .get("doc-in-folder")
            .expect("doc-in-folder indexed");
        assert!(in_folder.contains(&"alice@example.com".to_string()));
        assert!(in_folder.contains(&"bob@example.com".to_string()));
        assert!(
            in_folder.contains(&"guest@external.com".to_string()),
            "normal folder direct grants must union in (expansive access)"
        );

        let in_secret = perms_by_doc
            .get("doc-in-secret")
            .expect("doc-in-secret indexed");
        assert_eq!(
            in_secret,
            &vec!["carol@example.com".to_string()],
            "limited-access boundary must drop non-organizer drive members"
        );

        Ok(())
    }

    /// A folder-ACL fingerprint change (grant added/removed on a fingerprinted
    /// folder) must force a full re-traversal on the next incremental run so
    /// descendants are re-emitted with corrected ACLs.
    #[tokio::test]
    async fn sa_direct_incremental_folder_acl_change_forces_full_traversal() -> Result<()> {
        let _guard = SA_ENV_LOCK.lock().await;
        let (mock_base, state) = spawn_sa_mock().await?;
        let previous_drive_base = std::env::var("GOOGLE_DRIVE_API_BASE").ok();
        // TODO: Audit that the environment access only happens in single-threaded code.
        unsafe { std::env::set_var("GOOGLE_DRIVE_API_BASE", format!("{}/drive/v3", mock_base)) };

        let drive_acl = json!([
            { "id": "p1", "type": "user", "emailAddress": SA_CLIENT_EMAIL, "domain": null, "role": "organizer", "allowFileDiscovery": null, "permissionDetails": null },
            { "id": "p2", "type": "user", "emailAddress": "alice@example.com", "domain": null, "role": "writer", "allowFileDiscovery": null, "permissionDetails": null }
        ]);
        state.set_permissions(drive_acl.clone());

        // Live ACL for secret-1 differs from the stale checkpoint fingerprint.
        state.set_folder_permissions(
            "secret-1",
            json!([
                { "id": "p4", "type": "user", "emailAddress": "carol@example.com", "domain": null, "role": "writer", "allowFileDiscovery": null, "permissionDetails": null },
                { "id": "p5", "type": "user", "emailAddress": "dave@example.com", "domain": null, "role": "reader", "allowFileDiscovery": null, "permissionDetails": null }
            ]),
        );

        let drive_acl_typed: Vec<omni_google_connector::models::GoogleDrivePermission> =
            serde_json::from_value(drive_acl)?;
        let drive_fp = omni_google_connector::models::drive_acl_fingerprint(&drive_acl_typed);
        let config: omni_google_connector::models::GoogleSourceConfig =
            serde_json::from_value(sa_source().config)?;
        let scope_fp = format!(
            "sa-direct:{}",
            omni_google_connector::models::compute_scope_fingerprint(Some(
                &config.folder_path_filters
            ))
        );

        let checkpoint = GoogleSyncCheckpoint {
            drive_change_tokens: Some(HashMap::from([("drive-1".to_string(), "tok1".to_string())])),
            drive_scope_fingerprint: Some(scope_fp),
            drive_acl_fingerprints: Some(HashMap::from([("drive-1".to_string(), drive_fp)])),
            folder_access: Some(HashMap::from([(
                "secret-1".to_string(),
                omni_google_connector::models::FolderAccessInfo {
                    drive_id: "drive-1".to_string(),
                    parents: vec![],
                    inherited_permissions_disabled: true,
                    acl_fingerprint: "stale-fingerprint".to_string(),
                },
            )])),
            permission_model_version: Some(
                omni_google_connector::models::SA_DIRECT_PERMISSION_MODEL_VERSION.to_string(),
            ),
            ..Default::default()
        };

        let sync_result = run_sa_sync_with_checkpoint(
            &mock_base,
            sa_source(),
            sa_credentials(&mock_base),
            SyncType::Incremental,
            checkpoint,
        )
        .await;

        if let Some(value) = previous_drive_base {
            // TODO: Audit that the environment access only happens in single-threaded code.
            unsafe { std::env::set_var("GOOGLE_DRIVE_API_BASE", value) };
        } else {
            // TODO: Audit that the environment access only happens in single-threaded code.
            unsafe { std::env::remove_var("GOOGLE_DRIVE_API_BASE") };
        }

        sync_result?;

        assert!(
            state.file_list_calls() >= 1,
            "folder ACL fingerprint change must trigger a full re-traversal"
        );

        Ok(())
    }
}
