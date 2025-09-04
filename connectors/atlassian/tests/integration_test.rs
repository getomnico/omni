use anyhow::Result;
use chrono::Utc;
use shared::models::{ConnectorEvent, SourceType};
use shared::queue::EventQueue;
use shared::test_environment::TestEnvironment;
use tokio::test;

use omni_atlassian_connector::{
    AtlassianCredentials, AuthManager, ConfluenceProcessor, JiraProcessor, SyncManager,
};

const TEST_BASE_URL: &str = "https://test-company.atlassian.net";
const TEST_USER_EMAIL: &str = "test@example.com";
const TEST_API_TOKEN: &str = "test-api-token";

#[tokio::test]
async fn test_atlassian_credentials_validation() {
    let credentials = AtlassianCredentials::new(
        TEST_BASE_URL.to_string(),
        TEST_USER_EMAIL.to_string(),
        TEST_API_TOKEN.to_string(),
    );

    assert_eq!(credentials.base_url, TEST_BASE_URL);
    assert_eq!(credentials.user_email, TEST_USER_EMAIL);
    assert_eq!(credentials.api_token, TEST_API_TOKEN);
    assert!(credentials.is_valid());

    // Test auth header generation
    let auth_header = credentials.get_basic_auth_header();
    assert!(auth_header.starts_with("Basic "));
}

#[tokio::test]
async fn test_sync_state_operations() -> Result<()> {
    let test_env = TestEnvironment::new().await?;
    let redis_client = redis::Client::open(test_env.redis_url())?;

    let sync_manager = SyncManager::new(test_env.pool().clone(), redis_client).await?;
    let sync_state = sync_manager.get_sync_state();

    let source_id = "test-source-123";
    let space_key = "TEST";
    let project_key = "PROJ";

    // Test Confluence sync state
    let last_sync = sync_state
        .get_confluence_last_sync(source_id, space_key)
        .await?;
    assert!(last_sync.is_none());

    let now = Utc::now();
    sync_state
        .set_confluence_last_sync(source_id, space_key, now)
        .await?;

    let retrieved_sync = sync_state
        .get_confluence_last_sync(source_id, space_key)
        .await?;
    assert!(retrieved_sync.is_some());
    assert_eq!(retrieved_sync.unwrap().timestamp(), now.timestamp());

    // Test JIRA sync state
    let last_sync = sync_state
        .get_jira_last_sync(source_id, project_key)
        .await?;
    assert!(last_sync.is_none());

    sync_state
        .set_jira_last_sync(source_id, project_key, now)
        .await?;

    let retrieved_sync = sync_state
        .get_jira_last_sync(source_id, project_key)
        .await?;
    assert!(retrieved_sync.is_some());
    assert_eq!(retrieved_sync.unwrap().timestamp(), now.timestamp());

    // Test getting all synced resources
    let confluence_spaces = sync_state
        .get_all_synced_confluence_spaces(source_id)
        .await?;
    assert!(confluence_spaces.contains(space_key));

    let jira_projects = sync_state.get_all_synced_jira_projects(source_id).await?;
    assert!(jira_projects.contains(project_key));

    Ok(())
}

#[tokio::test]
async fn test_event_queue_integration() -> Result<()> {
    let test_env = TestEnvironment::new().await?;
    let event_queue = EventQueue::new(test_env.pool().clone());

    // Create a test Confluence event
    let confluence_event = ConnectorEvent::DocumentCreated {
        source_id: "test-source".to_string(),
        document_id: "confluence_page_TEST_123".to_string(),
        content: "This is a test Confluence page content".to_string(),
        metadata: shared::models::DocumentMetadata {
            title: Some("Test Page".to_string()),
            author: Some("Test Author".to_string()),
            created_at: None,
            updated_at: None,
            mime_type: Some("text/html".to_string()),
            size: Some("100".to_string()),
            url: Some("https://test.atlassian.net/wiki/spaces/TEST/pages/123".to_string()),
            parent_id: None,
            extra: None,
        },
        permissions: shared::models::DocumentPermissions {
            public: false,
            users: vec![],
            groups: vec!["confluence_space_TEST".to_string()],
        },
    };

    // Queue the event
    event_queue.enqueue_event(confluence_event, 3).await?;

    // Create a test JIRA event
    let jira_event = ConnectorEvent::DocumentCreated {
        source_id: "test-source".to_string(),
        document_id: "jira_issue_PROJ_ISSUE-123".to_string(),
        content: "Summary: Test Issue\n\nDescription: This is a test issue".to_string(),
        metadata: shared::models::DocumentMetadata {
            title: Some("ISSUE-123 - Test Issue".to_string()),
            author: Some("Test User".to_string()),
            created_at: None,
            updated_at: None,
            mime_type: Some("text/plain".to_string()),
            size: Some("50".to_string()),
            url: Some("https://test.atlassian.net/browse/ISSUE-123".to_string()),
            parent_id: None,
            extra: None,
        },
        permissions: shared::models::DocumentPermissions {
            public: false,
            users: vec![],
            groups: vec!["jira_project_PROJ".to_string()],
        },
    };

    // Queue the event
    event_queue.enqueue_event(jira_event, 3).await?;

    // Verify events were queued (we can't easily test processing without real indexer)
    // This test validates the queue integration works correctly
    println!("Successfully queued Confluence and JIRA events");

    Ok(())
}

#[tokio::test]
async fn test_confluence_processor_initialization() -> Result<()> {
    let test_env = TestEnvironment::new().await?;
    let event_queue = EventQueue::new(test_env.pool().clone());

    let confluence_processor = ConfluenceProcessor::new(event_queue);

    // Test rate limit info (should not crash)
    let rate_limit_info = confluence_processor.get_rate_limit_info();
    assert!(!rate_limit_info.is_empty());

    Ok(())
}

#[tokio::test]
async fn test_jira_processor_initialization() -> Result<()> {
    let test_env = TestEnvironment::new().await?;
    let event_queue = EventQueue::new(test_env.pool().clone());

    let jira_processor = JiraProcessor::new(event_queue);

    // Test rate limit info (should not crash)
    let rate_limit_info = jira_processor.get_rate_limit_info();
    assert!(!rate_limit_info.is_empty());

    Ok(())
}

#[tokio::test]
async fn test_document_mapping() {
    use omni_atlassian_connector::models::{
        ConfluencePage, ConfluenceSpace, ConfluenceUser, ConfluenceVersion, JiraFields, JiraIssue,
        JiraIssueType, JiraProject, JiraStatus, JiraStatusCategory,
    };

    // Test Confluence page document mapping
    let confluence_page = ConfluencePage {
        id: "123456".to_string(),
        title: "Test Page".to_string(),
        r#type: "page".to_string(),
        status: "current".to_string(),
        space: ConfluenceSpace {
            id: "98765".to_string(),
            key: "TEST".to_string(),
            name: "Test Space".to_string(),
            r#type: "global".to_string(),
        },
        version: ConfluenceVersion {
            by: ConfluenceUser {
                user_type: "known".to_string(),
                account_id: "user123".to_string(),
                display_name: "Test User".to_string(),
                email: Some("test@example.com".to_string()),
            },
            when: "2024-01-01T10:00:00.000Z".to_string(),
            number: 1,
            message: None,
        },
        body: None,
        ancestors: None,
        links: None,
    };

    let confluence_event =
        confluence_page.to_connector_event("source-123".to_string(), TEST_BASE_URL);

    match confluence_event {
        ConnectorEvent::DocumentCreated {
            document_id,
            metadata,
            permissions,
            ..
        } => {
            assert_eq!(document_id, "confluence_page_TEST_123456");
            assert_eq!(metadata.title, Some("Test Page".to_string()));
            assert_eq!(metadata.author, Some("Test User".to_string()));
            assert_eq!(permissions.groups, vec!["confluence_space_TEST"]);
        }
        _ => panic!("Expected DocumentCreated event"),
    }

    // Test JIRA issue document mapping
    let jira_issue = JiraIssue {
        id: "10001".to_string(),
        key: "PROJ-123".to_string(),
        self_url: "https://test.atlassian.net/rest/api/3/issue/10001".to_string(),
        fields: JiraFields {
            summary: "Test Issue".to_string(),
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
                key: "PROJ".to_string(),
                name: "Test Project".to_string(),
                avatar_urls: None,
            },
            created: "2024-01-01T10:00:00.000Z".to_string(),
            updated: "2024-01-01T10:00:00.000Z".to_string(),
            labels: None,
            comment: None,
            components: None,
        },
    };

    let jira_event = jira_issue.to_connector_event("source-123".to_string(), TEST_BASE_URL);

    match jira_event {
        ConnectorEvent::DocumentCreated {
            document_id,
            metadata,
            permissions,
            ..
        } => {
            assert_eq!(document_id, "jira_issue_PROJ_PROJ-123");
            assert_eq!(metadata.title, Some("PROJ-123 - Test Issue".to_string()));
            assert_eq!(permissions.groups, vec!["jira_project_PROJ"]);
        }
        _ => panic!("Expected DocumentCreated event"),
    }
}

#[tokio::test]
async fn test_auth_manager_creation() {
    let auth_manager = AuthManager::new();

    // This test just validates that the auth manager can be created without issues
    // Real authentication testing would require valid Atlassian credentials
    assert!(std::mem::size_of_val(&auth_manager) > 0);
}

#[tokio::test]
async fn test_sync_manager_creation() -> Result<()> {
    let test_env = TestEnvironment::new().await?;
    let redis_client = redis::Client::open(test_env.redis_url())?;

    let sync_manager = SyncManager::new(test_env.pool().clone(), redis_client).await?;

    // Test that sync manager can get its sync state
    let _sync_state = sync_manager.get_sync_state();

    Ok(())
}
