use anyhow::Result;
use async_trait::async_trait;
use omni_web_connector::config::WebSourceConfig;
use omni_web_connector::models::WebPage;
use omni_web_connector::sync::{CrawlResult, PageSource, SyncManager};
use shared::db::repositories::{SourceRepository, SyncRunRepository};
use shared::models::{SourceType, SyncStatus};
use shared::queue::EventQueue;
use shared::storage::postgres::PostgresStorage;
use shared::test_environment::TestEnvironment;
use sqlx::Row;
use std::sync::Arc;
use tokio::sync::mpsc;

/// Mock page source that returns predefined pages
struct MockPageSource {
    pages: Vec<WebPage>,
}

impl MockPageSource {
    fn new(pages: Vec<WebPage>) -> Self {
        Self { pages }
    }

    fn from_html_pages(pages: Vec<(&str, &str)>) -> Self {
        let web_pages: Vec<WebPage> = pages
            .into_iter()
            .filter_map(|(url, html)| WebPage::from_html(url.to_string(), html).ok())
            .collect();
        Self::new(web_pages)
    }
}

#[async_trait]
impl PageSource for MockPageSource {
    async fn crawl(
        &self,
        _config: &WebSourceConfig,
        tx: mpsc::Sender<WebPage>,
    ) -> Result<CrawlResult> {
        for page in &self.pages {
            if tx.send(page.clone()).await.is_err() {
                break;
            }
        }
        Ok(CrawlResult {
            pages_crawled: self.pages.len(),
        })
    }
}

/// Test fixture for web connector integration tests
struct WebConnectorTestFixture {
    test_env: TestEnvironment,
}

impl WebConnectorTestFixture {
    async fn new() -> Result<Self> {
        let test_env = TestEnvironment::new().await?;
        Ok(Self { test_env })
    }

    fn pool(&self) -> &sqlx::PgPool {
        self.test_env.db_pool.pool()
    }

    fn redis_client(&self) -> redis::Client {
        self.test_env.redis_client.clone()
    }

    async fn create_test_user(&self, email: &str) -> Result<String> {
        let user_id = shared::utils::generate_ulid();

        sqlx::query(
            "INSERT INTO users (id, email, full_name, role, password_hash) VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(&user_id)
        .bind(email)
        .bind("Test User")
        .bind("admin")
        .bind("hashed_password")
        .execute(self.pool())
        .await?;

        Ok(user_id)
    }

    async fn create_test_source(&self, name: &str, user_id: &str, root_url: &str) -> Result<String> {
        let source_id = shared::utils::generate_ulid();
        let config = serde_json::json!({
            "root_url": root_url,
            "max_depth": 2,
            "max_pages": 100
        });

        sqlx::query(
            "INSERT INTO sources (id, name, source_type, is_active, created_by, config) VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(&source_id)
        .bind(name)
        .bind(SourceType::Web)
        .bind(true)
        .bind(user_id)
        .bind(&config)
        .execute(self.pool())
        .await?;

        Ok(source_id)
    }

    async fn get_queued_events(&self, source_id: &str) -> Result<Vec<serde_json::Value>> {
        let rows = sqlx::query(
            "SELECT payload FROM connector_events_queue WHERE source_id = $1 ORDER BY created_at",
        )
        .bind(source_id)
        .fetch_all(self.pool())
        .await?;

        let events: Vec<serde_json::Value> = rows
            .iter()
            .map(|row| row.get::<serde_json::Value, _>("payload"))
            .collect();

        Ok(events)
    }

    async fn get_sync_run(&self, source_id: &str) -> Result<Option<shared::models::SyncRun>> {
        let sync_run_repo = SyncRunRepository::new(self.pool());
        let running = sync_run_repo
            .get_running_for_source(source_id)
            .await
            .map_err(|e| anyhow::anyhow!("{}", e))?;
        if running.is_some() {
            return Ok(running);
        }
        // Get latest completed
        sync_run_repo
            .get_last_completed_for_source(source_id, shared::models::SyncType::Full)
            .await
            .map_err(|e| anyhow::anyhow!("{}", e))
    }

    fn create_sync_manager(&self, page_source: Arc<dyn PageSource>) -> SyncManager {
        let event_queue = EventQueue::new(self.pool().clone());
        let content_storage = Arc::new(PostgresStorage::new(self.pool().clone()));
        let sync_run_repo = SyncRunRepository::new(self.pool());
        let source_repo = SourceRepository::new(self.pool());

        SyncManager::new_for_testing(
            self.redis_client(),
            event_queue,
            content_storage,
            sync_run_repo,
            source_repo,
            page_source,
        )
    }
}

fn create_test_html(title: &str, content: &str) -> String {
    format!(
        r#"<!DOCTYPE html>
<html>
<head><title>{}</title></head>
<body>
    <h1>{}</h1>
    <p>{}</p>
</body>
</html>"#,
        title, title, content
    )
}

#[tokio::test]
async fn test_sync_creates_events_for_crawled_pages() -> Result<()> {
    let fixture = WebConnectorTestFixture::new().await?;

    // Create test data
    let user_id = fixture.create_test_user("test@example.com").await?;
    let source_id = fixture
        .create_test_source("Test Website", &user_id, "https://example.com")
        .await?;

    // Create mock pages
    let mock_pages = MockPageSource::from_html_pages(vec![
        (
            "https://example.com/",
            &create_test_html("Home", "Welcome to our website"),
        ),
        (
            "https://example.com/about",
            &create_test_html("About", "About our company"),
        ),
        (
            "https://example.com/contact",
            &create_test_html("Contact", "Get in touch with us"),
        ),
    ]);

    // Create sync manager with mock page source
    let sync_manager = fixture.create_sync_manager(Arc::new(mock_pages));

    // Trigger sync
    sync_manager.sync_source_by_id(&source_id).await?;

    // Verify events were created
    let events = fixture.get_queued_events(&source_id).await?;
    assert_eq!(events.len(), 3, "Expected 3 events for 3 pages");

    // Verify event structure
    for event in &events {
        assert!(event.get("DocumentCreated").is_some(), "Event should be DocumentCreated");
        let doc_created = &event["DocumentCreated"];
        assert_eq!(doc_created["source_id"], source_id);
        assert!(doc_created["document_id"].as_str().is_some());
        assert!(doc_created["content_id"].as_str().is_some());
        assert!(doc_created["metadata"]["title"].as_str().is_some());
        assert_eq!(doc_created["metadata"]["mime_type"], "text/html");
    }

    // Verify sync run completed
    let sync_run = fixture.get_sync_run(&source_id).await?;
    assert!(sync_run.is_some());
    let sync_run = sync_run.unwrap();
    assert_eq!(sync_run.status, SyncStatus::Completed);
    assert_eq!(sync_run.documents_scanned, 3);
    assert_eq!(sync_run.documents_updated, 3);

    Ok(())
}

#[tokio::test]
async fn test_sync_skips_unchanged_pages_on_resync() -> Result<()> {
    let fixture = WebConnectorTestFixture::new().await?;

    let user_id = fixture.create_test_user("test2@example.com").await?;
    let source_id = fixture
        .create_test_source("Test Website 2", &user_id, "https://example2.com")
        .await?;

    // First sync with initial pages
    let initial_pages = MockPageSource::from_html_pages(vec![
        (
            "https://example2.com/page1",
            &create_test_html("Page 1", "Initial content"),
        ),
        (
            "https://example2.com/page2",
            &create_test_html("Page 2", "More content"),
        ),
    ]);

    let sync_manager = fixture.create_sync_manager(Arc::new(initial_pages));
    sync_manager.sync_source_by_id(&source_id).await?;

    let events_after_first_sync = fixture.get_queued_events(&source_id).await?;
    assert_eq!(events_after_first_sync.len(), 2, "First sync should create 2 events");

    // Second sync with same pages (unchanged)
    let same_pages = MockPageSource::from_html_pages(vec![
        (
            "https://example2.com/page1",
            &create_test_html("Page 1", "Initial content"),
        ),
        (
            "https://example2.com/page2",
            &create_test_html("Page 2", "More content"),
        ),
    ]);

    let sync_manager = fixture.create_sync_manager(Arc::new(same_pages));
    sync_manager.sync_source_by_id(&source_id).await?;

    // Should still have only 2 events (no new events for unchanged pages)
    let events_after_second_sync = fixture.get_queued_events(&source_id).await?;
    assert_eq!(
        events_after_second_sync.len(),
        2,
        "Second sync should not create new events for unchanged pages"
    );

    Ok(())
}

#[tokio::test]
async fn test_sync_creates_events_for_updated_pages() -> Result<()> {
    let fixture = WebConnectorTestFixture::new().await?;

    let user_id = fixture.create_test_user("test3@example.com").await?;
    let source_id = fixture
        .create_test_source("Test Website 3", &user_id, "https://example3.com")
        .await?;

    // First sync
    let initial_pages = MockPageSource::from_html_pages(vec![(
        "https://example3.com/page",
        &create_test_html("Page", "Initial content"),
    )]);

    let sync_manager = fixture.create_sync_manager(Arc::new(initial_pages));
    sync_manager.sync_source_by_id(&source_id).await?;

    let events_after_first_sync = fixture.get_queued_events(&source_id).await?;
    assert_eq!(events_after_first_sync.len(), 1);

    // Second sync with updated content
    let updated_pages = MockPageSource::from_html_pages(vec![(
        "https://example3.com/page",
        &create_test_html("Page", "Updated content - this is new!"),
    )]);

    let sync_manager = fixture.create_sync_manager(Arc::new(updated_pages));
    sync_manager.sync_source_by_id(&source_id).await?;

    // Should now have 2 events (original + update)
    let events_after_second_sync = fixture.get_queued_events(&source_id).await?;
    assert_eq!(
        events_after_second_sync.len(),
        2,
        "Should create new event for updated page"
    );

    Ok(())
}

#[tokio::test]
async fn test_sync_creates_deletion_events_for_removed_pages() -> Result<()> {
    let fixture = WebConnectorTestFixture::new().await?;

    let user_id = fixture.create_test_user("test4@example.com").await?;
    let source_id = fixture
        .create_test_source("Test Website 4", &user_id, "https://example4.com")
        .await?;

    // First sync with 2 pages
    let initial_pages = MockPageSource::from_html_pages(vec![
        (
            "https://example4.com/keep",
            &create_test_html("Keep", "This stays"),
        ),
        (
            "https://example4.com/remove",
            &create_test_html("Remove", "This will be removed"),
        ),
    ]);

    let sync_manager = fixture.create_sync_manager(Arc::new(initial_pages));
    sync_manager.sync_source_by_id(&source_id).await?;

    let events_after_first_sync = fixture.get_queued_events(&source_id).await?;
    assert_eq!(events_after_first_sync.len(), 2);

    // Second sync with only 1 page (one removed)
    let remaining_pages = MockPageSource::from_html_pages(vec![(
        "https://example4.com/keep",
        &create_test_html("Keep", "This stays"),
    )]);

    let sync_manager = fixture.create_sync_manager(Arc::new(remaining_pages));
    sync_manager.sync_source_by_id(&source_id).await?;

    // Should have 3 events: 2 creates + 1 delete
    let events_after_second_sync = fixture.get_queued_events(&source_id).await?;
    assert_eq!(
        events_after_second_sync.len(),
        3,
        "Should create deletion event for removed page"
    );

    // Verify the last event is a deletion
    let last_event = events_after_second_sync.last().unwrap();
    assert!(
        last_event.get("DocumentDeleted").is_some(),
        "Last event should be DocumentDeleted"
    );

    Ok(())
}

#[tokio::test]
async fn test_event_contains_correct_metadata() -> Result<()> {
    let fixture = WebConnectorTestFixture::new().await?;

    let user_id = fixture.create_test_user("test5@example.com").await?;
    let source_id = fixture
        .create_test_source("Test Website 5", &user_id, "https://example5.com")
        .await?;

    let html = r#"<!DOCTYPE html>
<html>
<head>
    <title>Test Page Title</title>
    <meta name="description" content="This is a test description">
</head>
<body>
    <h1>Main Heading</h1>
    <p>Paragraph one with some content.</p>
    <p>Paragraph two with more content.</p>
</body>
</html>"#;

    let mock_pages = MockPageSource::from_html_pages(vec![("https://example5.com/test-page", html)]);

    let sync_manager = fixture.create_sync_manager(Arc::new(mock_pages));
    sync_manager.sync_source_by_id(&source_id).await?;

    let events = fixture.get_queued_events(&source_id).await?;
    assert_eq!(events.len(), 1);

    let event = &events[0]["DocumentCreated"];
    let metadata = &event["metadata"];

    // Verify metadata fields
    assert_eq!(metadata["title"], "Test Page Title");
    assert_eq!(metadata["mime_type"], "text/html");
    assert_eq!(metadata["url"], "https://example5.com/test-page");
    assert_eq!(metadata["path"], "/test-page");

    // Verify extra metadata
    let extra = &metadata["extra"];
    assert_eq!(extra["domain"], "example5.com");
    assert!(extra["word_count"].as_i64().unwrap() > 0);
    assert!(extra["content_hash"].as_str().is_some());

    Ok(())
}
