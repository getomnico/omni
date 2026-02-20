mod common;

use anyhow::Result;
use common::{
    count_queued_events, get_queued_events, get_queued_events_by_type, setup_test_fixture,
    TEST_API_TOKEN, TEST_BASE_URL, TEST_USER_EMAIL,
};
use omni_atlassian_connector::models::{
    AtlassianWebhookEvent, AtlassianWebhookIssue, AtlassianWebhookIssueFields,
    AtlassianWebhookPage, AtlassianWebhookProject, AtlassianWebhookSpace, ConfluenceContent,
    ConfluenceCqlBody, ConfluenceCqlPage, ConfluenceCqlSpace, ConfluenceCqlVersion, ConfluencePage,
    ConfluencePageBody, ConfluencePageLinks, ConfluencePageStatus, ConfluenceSpace,
    ConfluenceVersion, JiraFields, JiraIssue, JiraIssueType, JiraProject, JiraSearchResponse,
    JiraStatus, JiraStatusCategory,
};
use omni_atlassian_connector::{
    AtlassianCredentials, ConfluenceProcessor, JiraProcessor, SyncManager,
};
use shared::models::SourceType;
use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use time::OffsetDateTime;

const SOURCE_ID: &str = "01JGF7V3E0Y2R1X8P5Q7W9T4N7";

fn test_credentials() -> AtlassianCredentials {
    AtlassianCredentials::new(
        TEST_BASE_URL.to_string(),
        TEST_USER_EMAIL.to_string(),
        TEST_API_TOKEN.to_string(),
    )
}

fn make_confluence_space(id: &str, key: &str, name: &str) -> ConfluenceSpace {
    ConfluenceSpace {
        id: id.to_string(),
        key: key.to_string(),
        name: name.to_string(),
        r#type: "global".to_string(),
    }
}

fn make_confluence_page(id: &str, title: &str, space_id: &str, version: i32) -> ConfluencePage {
    ConfluencePage {
        id: id.to_string(),
        status: ConfluencePageStatus::Current,
        title: title.to_string(),
        space_id: space_id.to_string(),
        parent_id: None,
        parent_type: None,
        position: None,
        author_id: "user123".to_string(),
        owner_id: None,
        last_owner_id: None,
        subtype: None,
        created_at: OffsetDateTime::now_utc(),
        version: ConfluenceVersion {
            created_at: OffsetDateTime::now_utc(),
            message: String::new(),
            number: version,
            minor_edit: false,
            author_id: "user123".to_string(),
        },
        body: Some(ConfluencePageBody {
            storage: Some(ConfluenceContent {
                value: format!("<p>Content of {}</p>", title),
                representation: "storage".to_string(),
            }),
            atlas_doc_format: None,
        }),
        links: ConfluencePageLinks {
            webui: format!("/spaces/TEST/pages/{}/{}", id, title.replace(' ', "+")),
            editui: String::new(),
            tinyui: String::new(),
        },
    }
}

fn make_cql_page(
    id: &str,
    title: &str,
    space_id: i64,
    space_key: &str,
    version: i32,
) -> ConfluenceCqlPage {
    ConfluenceCqlPage {
        id: id.to_string(),
        title: title.to_string(),
        status: "current".to_string(),
        content_type: "page".to_string(),
        space: Some(ConfluenceCqlSpace {
            id: Some(space_id),
            key: space_key.to_string(),
            name: format!("{} Space", space_key),
        }),
        version: Some(ConfluenceCqlVersion {
            number: version,
            when: "2024-06-15T10:00:00.000Z".to_string(),
            minor_edit: false,
        }),
        body: Some(ConfluenceCqlBody {
            storage: Some(ConfluenceContent {
                value: format!("<p>CQL Content of {}</p>", title),
                representation: "storage".to_string(),
            }),
        }),
        links: None,
    }
}

fn make_jira_issue(key: &str, summary: &str, project_key: &str) -> JiraIssue {
    JiraIssue {
        id: "10001".to_string(),
        key: key.to_string(),
        self_url: format!("{}/rest/api/3/issue/10001", TEST_BASE_URL),
        fields: JiraFields {
            summary: summary.to_string(),
            description: None,
            issuetype: JiraIssueType {
                id: "1".to_string(),
                name: "Bug".to_string(),
                icon_url: None,
            },
            status: JiraStatus {
                id: "1".to_string(),
                name: "Open".to_string(),
                status_category: JiraStatusCategory {
                    id: 1,
                    name: "New".to_string(),
                    key: "new".to_string(),
                    color_name: "blue-gray".to_string(),
                },
            },
            priority: None,
            assignee: None,
            reporter: None,
            creator: None,
            project: JiraProject {
                id: "10000".to_string(),
                key: project_key.to_string(),
                name: "Test Project".to_string(),
                avatar_urls: None,
            },
            created: "2024-01-01T10:00:00.000+0000".to_string(),
            updated: "2024-01-01T10:00:00.000+0000".to_string(),
            labels: None,
            comment: None,
            components: None,
            extra_fields: HashMap::new(),
        },
    }
}

// =============================================================================
// Confluence Sync Tests
// =============================================================================

#[tokio::test]
async fn test_confluence_full_sync_creates_events() -> Result<()> {
    let fixture = setup_test_fixture(SourceType::Confluence).await?;

    // Set up mock: 2 spaces, each with 2 pages
    *fixture.mock_api.spaces.lock().unwrap() = vec![
        make_confluence_space("100", "DEV", "Development"),
        make_confluence_space("200", "OPS", "Operations"),
    ];

    *fixture.mock_api.pages.lock().unwrap() = vec![
        vec![
            make_confluence_page("1001", "Dev Page 1", "100", 1),
            make_confluence_page("1002", "Dev Page 2", "100", 1),
        ],
        vec![
            make_confluence_page("2001", "Ops Page 1", "200", 1),
            make_confluence_page("2002", "Ops Page 2", "200", 1),
        ],
    ];

    let redis_url = fixture.state.config.redis.redis_url.clone();
    let redis_client = redis::Client::open(redis_url)?;
    let mut processor = ConfluenceProcessor::new(
        fixture.mock_api.clone(),
        fixture.sdk_client.clone(),
        redis_client,
    );

    let cancelled = AtomicBool::new(false);
    let sync_run_id = fixture
        .sdk_client
        .create_sync_run(SOURCE_ID, shared::models::SyncType::Full)
        .await?;

    let creds = test_credentials();
    let count = processor
        .sync_all_spaces(&creds, SOURCE_ID, &sync_run_id, &cancelled)
        .await?;

    assert_eq!(count, 4, "Should process 4 pages across 2 spaces");

    let events = get_queued_events(&fixture.pool).await?;
    assert_eq!(events.len(), 4, "Should have 4 events in queue");

    for event in &events {
        assert_eq!(event["type"], "document_created");
        assert_eq!(event["source_id"], SOURCE_ID);
    }

    // Verify mock was called correctly
    let space_calls = fixture.mock_api.get_calls_for("get_confluence_spaces");
    assert_eq!(space_calls.len(), 1);

    let page_calls = fixture.mock_api.get_calls_for("get_confluence_pages");
    assert_eq!(page_calls.len(), 2, "Should fetch pages for each space");

    Ok(())
}

#[tokio::test]
async fn test_confluence_incremental_sync_uses_cql() -> Result<()> {
    let fixture = setup_test_fixture(SourceType::Confluence).await?;

    // Set up mock: CQL search returns 1 modified page
    *fixture.mock_api.cql_pages.lock().unwrap() =
        vec![make_cql_page("3001", "Modified Page", 100, "DEV", 5)];

    let redis_url = fixture.state.config.redis.redis_url.clone();
    let redis_client = redis::Client::open(redis_url)?;
    let mut processor = ConfluenceProcessor::new(
        fixture.mock_api.clone(),
        fixture.sdk_client.clone(),
        redis_client,
    );

    let cancelled = AtomicBool::new(false);
    let sync_run_id = fixture
        .sdk_client
        .create_sync_run(SOURCE_ID, shared::models::SyncType::Incremental)
        .await?;

    let creds = test_credentials();
    let last_sync = chrono::Utc::now() - chrono::Duration::hours(1);

    let count = processor
        .sync_all_spaces_incremental(&creds, SOURCE_ID, &sync_run_id, last_sync, &cancelled)
        .await?;

    assert_eq!(count, 1, "Should process 1 modified page");

    // Verify CQL search was used (not full page listing)
    let cql_calls = fixture
        .mock_api
        .get_calls_for("search_confluence_pages_by_cql");
    assert_eq!(cql_calls.len(), 1, "Should use CQL search");

    let full_page_calls = fixture.mock_api.get_calls_for("get_confluence_pages");
    assert_eq!(full_page_calls.len(), 0, "Should NOT use full page listing");

    let events = get_queued_events(&fixture.pool).await?;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["type"], "document_created");

    Ok(())
}

#[tokio::test]
async fn test_confluence_version_dedup_skips_unchanged() -> Result<()> {
    let fixture = setup_test_fixture(SourceType::Confluence).await?;

    // Set up mock: 1 space with 2 pages
    *fixture.mock_api.spaces.lock().unwrap() =
        vec![make_confluence_space("100", "DEV", "Development")];

    *fixture.mock_api.pages.lock().unwrap() = vec![vec![
        make_confluence_page("1001", "Page 1", "100", 1),
        make_confluence_page("1002", "Page 2", "100", 1),
    ]];

    let redis_url = fixture.state.config.redis.redis_url.clone();
    let redis_client = redis::Client::open(redis_url)?;
    let mut processor = ConfluenceProcessor::new(
        fixture.mock_api.clone(),
        fixture.sdk_client.clone(),
        redis_client,
    );

    let cancelled = AtomicBool::new(false);

    // First sync: should process both pages
    let sync_run_id = fixture
        .sdk_client
        .create_sync_run(SOURCE_ID, shared::models::SyncType::Full)
        .await?;

    let creds = test_credentials();
    let count = processor
        .sync_all_spaces(&creds, SOURCE_ID, &sync_run_id, &cancelled)
        .await?;
    assert_eq!(count, 2, "First sync should process 2 pages");

    let events_after_first = count_queued_events(&fixture.pool).await?;
    assert_eq!(events_after_first, 2);

    // Second sync with same versions: should skip both pages
    let sync_run_id2 = fixture
        .sdk_client
        .create_sync_run(SOURCE_ID, shared::models::SyncType::Full)
        .await?;

    let count2 = processor
        .sync_all_spaces(&creds, SOURCE_ID, &sync_run_id2, &cancelled)
        .await?;
    assert_eq!(count2, 0, "Second sync should skip unchanged pages");

    let events_after_second = count_queued_events(&fixture.pool).await?;
    assert_eq!(events_after_second, 2, "No new events should be created");

    Ok(())
}

// =============================================================================
// Jira Sync Tests
// =============================================================================

#[tokio::test]
async fn test_jira_full_sync_creates_events() -> Result<()> {
    let fixture = setup_test_fixture(SourceType::Jira).await?;

    // Set up mock: 1 project with 3 issues
    *fixture.mock_api.jira_projects.lock().unwrap() = vec![serde_json::json!({
        "key": "PROJ",
        "name": "Test Project",
    })];

    *fixture.mock_api.jira_search_response.lock().unwrap() = Some(JiraSearchResponse {
        issues: vec![
            make_jira_issue("PROJ-1", "First Issue", "PROJ"),
            make_jira_issue("PROJ-2", "Second Issue", "PROJ"),
            make_jira_issue("PROJ-3", "Third Issue", "PROJ"),
        ],
        is_last: true,
        next_page_token: None,
    });

    let mut processor = JiraProcessor::new(fixture.mock_api.clone(), fixture.sdk_client.clone());

    let cancelled = AtomicBool::new(false);
    let sync_run_id = fixture
        .sdk_client
        .create_sync_run(SOURCE_ID, shared::models::SyncType::Full)
        .await?;

    let creds = test_credentials();
    let count = processor
        .sync_all_projects(&creds, SOURCE_ID, &sync_run_id, &cancelled)
        .await?;

    assert_eq!(count, 3, "Should process 3 issues");

    let events = get_queued_events(&fixture.pool).await?;
    assert_eq!(events.len(), 3, "Should have 3 events in queue");

    for event in &events {
        assert_eq!(event["type"], "document_created");
        assert_eq!(event["source_id"], SOURCE_ID);
        assert!(event["document_id"]
            .as_str()
            .unwrap()
            .starts_with("jira_issue_PROJ_"));
    }

    // Verify mock calls
    let project_calls = fixture.mock_api.get_calls_for("get_jira_projects");
    assert_eq!(project_calls.len(), 1);

    let issue_calls = fixture.mock_api.get_calls_for("get_jira_issues");
    assert!(issue_calls.len() >= 1, "Should fetch issues");

    Ok(())
}

// =============================================================================
// Webhook Handler Tests
// =============================================================================

#[tokio::test]
async fn test_webhook_delete_jira_issue() -> Result<()> {
    let fixture = setup_test_fixture(SourceType::Jira).await?;

    let redis_url = fixture.state.config.redis.redis_url.clone();
    let redis_client = redis::Client::open(redis_url)?;

    let mut sync_manager = SyncManager::with_client(
        fixture.mock_api.clone(),
        redis_client,
        fixture.sdk_client.clone(),
        None,
    );

    let event = AtlassianWebhookEvent {
        webhook_event: "jira:issue_deleted".to_string(),
        issue: Some(AtlassianWebhookIssue {
            id: "10001".to_string(),
            key: "PROJ-99".to_string(),
            fields: Some(AtlassianWebhookIssueFields {
                project: Some(AtlassianWebhookProject {
                    key: "PROJ".to_string(),
                }),
            }),
        }),
        page: None,
    };

    sync_manager.handle_webhook_event(SOURCE_ID, event).await?;

    let delete_events = get_queued_events_by_type(&fixture.pool, "document_deleted").await?;
    assert_eq!(delete_events.len(), 1, "Should create 1 delete event");
    assert_eq!(delete_events[0]["document_id"], "jira_issue_PROJ_PROJ-99");

    Ok(())
}

#[tokio::test]
async fn test_webhook_delete_confluence_page() -> Result<()> {
    let fixture = setup_test_fixture(SourceType::Confluence).await?;

    let redis_url = fixture.state.config.redis.redis_url.clone();
    let redis_client = redis::Client::open(redis_url)?;

    let mut sync_manager = SyncManager::with_client(
        fixture.mock_api.clone(),
        redis_client,
        fixture.sdk_client.clone(),
        None,
    );

    let event = AtlassianWebhookEvent {
        webhook_event: "page_trashed".to_string(),
        issue: None,
        page: Some(AtlassianWebhookPage {
            id: "54321".to_string(),
            space_key: Some("TEAM".to_string()),
            space: Some(AtlassianWebhookSpace {
                key: "TEAM".to_string(),
            }),
        }),
    };

    sync_manager.handle_webhook_event(SOURCE_ID, event).await?;

    let delete_events = get_queued_events_by_type(&fixture.pool, "document_deleted").await?;
    assert_eq!(delete_events.len(), 1, "Should create 1 delete event");
    assert_eq!(
        delete_events[0]["document_id"],
        "confluence_page_TEAM_54321"
    );

    Ok(())
}

#[tokio::test]
async fn test_webhook_create_triggers_notify() -> Result<()> {
    let fixture = setup_test_fixture(SourceType::Jira).await?;

    let redis_url = fixture.state.config.redis.redis_url.clone();
    let redis_client = redis::Client::open(redis_url)?;

    let mut sync_manager = SyncManager::with_client(
        fixture.mock_api.clone(),
        redis_client,
        fixture.sdk_client.clone(),
        None,
    );

    let event = AtlassianWebhookEvent {
        webhook_event: "jira:issue_created".to_string(),
        issue: Some(AtlassianWebhookIssue {
            id: "10001".to_string(),
            key: "PROJ-42".to_string(),
            fields: Some(AtlassianWebhookIssueFields {
                project: Some(AtlassianWebhookProject {
                    key: "PROJ".to_string(),
                }),
            }),
        }),
        page: None,
    };

    // notify_webhook triggers the connector-manager to create a sync run and then
    // call the connector. The connector call will fail (dummy URL), but the sync run
    // is still created. We tolerate the error and verify the sync run exists.
    let _ = sync_manager.handle_webhook_event(SOURCE_ID, event).await;

    let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM sync_runs WHERE source_id = $1")
        .bind(SOURCE_ID)
        .fetch_one(&fixture.pool)
        .await?;

    assert!(row.0 >= 1, "notify_webhook should create a sync run");

    Ok(())
}

// =============================================================================
// Webhook Registration Tests
// =============================================================================

#[tokio::test]
async fn test_webhook_registration_after_sync() -> Result<()> {
    let fixture = setup_test_fixture(SourceType::Confluence).await?;

    *fixture.mock_api.webhook_register_result.lock().unwrap() = Some(42);

    let redis_url = fixture.state.config.redis.redis_url.clone();
    let redis_client = redis::Client::open(redis_url)?;

    let sync_manager = SyncManager::with_client(
        fixture.mock_api.clone(),
        redis_client,
        fixture.sdk_client.clone(),
        Some("https://example.com/webhook".to_string()),
    );

    let creds = test_credentials();
    sync_manager
        .ensure_webhook_registered(SOURCE_ID, &creds)
        .await?;

    let register_calls = fixture.mock_api.get_calls_for("register_webhook");
    assert_eq!(register_calls.len(), 1);
    assert!(
        register_calls[0].args[0].contains("source_id="),
        "Webhook URL should contain source_id"
    );

    // Verify connector state was saved with webhook_id
    let state = fixture.sdk_client.get_connector_state(SOURCE_ID).await?;
    assert!(state.is_some(), "Connector state should be saved");
    let state_val = state.unwrap();
    assert_eq!(state_val["webhook_id"], 42);

    Ok(())
}

#[tokio::test]
async fn test_webhook_reregistration_on_missing() -> Result<()> {
    let fixture = setup_test_fixture(SourceType::Confluence).await?;

    // Save connector state with existing webhook_id
    fixture
        .sdk_client
        .save_connector_state(SOURCE_ID, serde_json::json!({"webhook_id": 999}))
        .await?;

    // Mock: get_webhook returns false (webhook doesn't exist anymore)
    *fixture.mock_api.webhook_exists.lock().unwrap() = false;
    *fixture.mock_api.webhook_register_result.lock().unwrap() = Some(1000);

    let redis_url = fixture.state.config.redis.redis_url.clone();
    let redis_client = redis::Client::open(redis_url)?;

    let sync_manager = SyncManager::with_client(
        fixture.mock_api.clone(),
        redis_client,
        fixture.sdk_client.clone(),
        Some("https://example.com/webhook".to_string()),
    );

    let creds = test_credentials();
    sync_manager
        .ensure_webhook_registered(SOURCE_ID, &creds)
        .await?;

    // Verify get_webhook was called to check existing
    let get_calls = fixture.mock_api.get_calls_for("get_webhook");
    assert_eq!(get_calls.len(), 1);
    assert_eq!(get_calls[0].args[0], "999");

    // Verify register_webhook was called (re-registration)
    let register_calls = fixture.mock_api.get_calls_for("register_webhook");
    assert_eq!(register_calls.len(), 1);

    // Verify new webhook_id was saved
    let state = fixture
        .sdk_client
        .get_connector_state(SOURCE_ID)
        .await?
        .unwrap();
    assert_eq!(state["webhook_id"], 1000);

    Ok(())
}
