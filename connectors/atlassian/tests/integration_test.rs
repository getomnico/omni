use anyhow::Result;
use chrono::Utc;
use shared::models::ConnectorEvent;
use shared::test_environment::TestEnvironment;
use std::collections::HashMap;
use time::OffsetDateTime;

use omni_atlassian_connector::models::{
    AtlassianWebhookEvent, ConfluenceContent, ConfluenceCqlPage, ConfluenceCqlSpace,
    ConfluenceCqlVersion, ConfluencePage, ConfluencePageBody, ConfluencePageLinks,
    ConfluencePageStatus, ConfluenceVersion, JiraFields, JiraIssue, JiraIssueType, JiraProject,
    JiraStatus, JiraStatusCategory,
};

const TEST_BASE_URL: &str = "https://test-company.atlassian.net";

#[tokio::test]
async fn test_sync_state_operations() -> Result<()> {
    let test_env = TestEnvironment::new().await?;
    let redis_client = test_env.redis_client.clone();

    let sync_state = omni_atlassian_connector::sync::SyncState::new(redis_client);

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

    // Test page version tracking
    let page_version = sync_state
        .get_confluence_page_version(source_id, "space1", "page1")
        .await?;
    assert!(page_version.is_none());

    sync_state
        .set_confluence_page_version(source_id, "space1", "page1", 5)
        .await?;

    let page_version = sync_state
        .get_confluence_page_version(source_id, "space1", "page1")
        .await?;
    assert_eq!(page_version, Some(5));

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
async fn test_confluence_page_to_connector_event() {
    let page = make_test_confluence_page();

    let event = page.to_connector_event(
        "sync-run-1".to_string(),
        "source-123".to_string(),
        TEST_BASE_URL,
        "content-abc".to_string(),
    );

    match event {
        ConnectorEvent::DocumentCreated {
            sync_run_id,
            source_id,
            document_id,
            content_id,
            metadata,
            ..
        } => {
            assert_eq!(sync_run_id, "sync-run-1");
            assert_eq!(source_id, "source-123");
            assert_eq!(document_id, "confluence_page_98765_123456");
            assert_eq!(content_id, "content-abc");
            assert_eq!(metadata.title, Some("Test Page".to_string()));
            assert!(metadata.url.unwrap().contains("/wiki"));
        }
        _ => panic!("Expected DocumentCreated event"),
    }
}

#[tokio::test]
async fn test_jira_issue_to_connector_event() {
    let issue = make_test_jira_issue();

    let event = issue.to_connector_event(
        "sync-run-2".to_string(),
        "source-456".to_string(),
        TEST_BASE_URL,
        "content-def".to_string(),
    );

    match event {
        ConnectorEvent::DocumentCreated {
            document_id,
            metadata,
            attributes,
            ..
        } => {
            assert_eq!(document_id, "jira_issue_PROJ_PROJ-123");
            assert_eq!(metadata.title, Some("PROJ-123 - Test Issue".to_string()));
            assert!(metadata.url.unwrap().contains("/browse/PROJ-123"));

            let attrs = attributes.unwrap();
            assert_eq!(attrs.get("issue_type").unwrap(), "Bug");
            assert_eq!(attrs.get("status").unwrap(), "Open");
            assert_eq!(attrs.get("project_key").unwrap(), "PROJ");
        }
        _ => panic!("Expected DocumentCreated event"),
    }
}

#[tokio::test]
async fn test_confluence_page_extract_plain_text() {
    let mut page = make_test_confluence_page();
    page.body = Some(ConfluencePageBody {
        storage: Some(ConfluenceContent {
            value: "<p>Hello <strong>world</strong></p>".to_string(),
            representation: "storage".to_string(),
        }),
        atlas_doc_format: None,
    });

    let text = page.extract_plain_text();
    assert_eq!(text, "Hello world");
}

#[tokio::test]
async fn test_cql_page_conversion() {
    let cql_page = ConfluenceCqlPage {
        id: "111".to_string(),
        title: "CQL Page".to_string(),
        status: "current".to_string(),
        content_type: "page".to_string(),
        space: Some(ConfluenceCqlSpace {
            id: Some(222),
            key: "DEV".to_string(),
            name: "Development".to_string(),
        }),
        version: Some(ConfluenceCqlVersion {
            number: 3,
            when: "2024-06-15T10:00:00.000Z".to_string(),
            minor_edit: false,
        }),
        body: None,
        links: None,
    };

    let page = cql_page.into_confluence_page();
    assert!(page.is_some());

    let page = page.unwrap();
    assert_eq!(page.id, "111");
    assert_eq!(page.title, "CQL Page");
    assert_eq!(page.space_id, "222");
    assert_eq!(page.version.number, 3);
    assert_eq!(page.status, ConfluencePageStatus::Current);
}

#[tokio::test]
async fn test_cql_page_conversion_without_space_returns_none() {
    let cql_page = ConfluenceCqlPage {
        id: "111".to_string(),
        title: "No Space Page".to_string(),
        status: "current".to_string(),
        content_type: "page".to_string(),
        space: None,
        version: Some(ConfluenceCqlVersion {
            number: 1,
            when: "2024-01-01T00:00:00.000Z".to_string(),
            minor_edit: false,
        }),
        body: None,
        links: None,
    };

    assert!(cql_page.into_confluence_page().is_none());
}

#[tokio::test]
async fn test_cql_page_conversion_without_version_returns_none() {
    let cql_page = ConfluenceCqlPage {
        id: "111".to_string(),
        title: "No Version Page".to_string(),
        status: "current".to_string(),
        content_type: "page".to_string(),
        space: Some(ConfluenceCqlSpace {
            id: Some(222),
            key: "DEV".to_string(),
            name: "Development".to_string(),
        }),
        version: None,
        body: None,
        links: None,
    };

    assert!(cql_page.into_confluence_page().is_none());
}

#[tokio::test]
async fn test_webhook_event_deserialization_jira_issue() {
    let json = serde_json::json!({
        "webhookEvent": "jira:issue_updated",
        "issue": {
            "id": "10001",
            "key": "PROJ-42",
            "fields": {
                "project": {
                    "key": "PROJ"
                }
            }
        }
    });

    let event: AtlassianWebhookEvent = serde_json::from_value(json).unwrap();
    assert_eq!(event.webhook_event, "jira:issue_updated");
    assert!(event.issue.is_some());
    assert!(event.page.is_none());

    let issue = event.issue.unwrap();
    assert_eq!(issue.key, "PROJ-42");
    assert_eq!(issue.fields.unwrap().project.unwrap().key, "PROJ");
}

#[tokio::test]
async fn test_webhook_event_deserialization_confluence_page() {
    let json = serde_json::json!({
        "webhookEvent": "page_updated",
        "page": {
            "id": "98765",
            "spaceKey": "DEV",
            "space": {
                "key": "DEV"
            }
        }
    });

    let event: AtlassianWebhookEvent = serde_json::from_value(json).unwrap();
    assert_eq!(event.webhook_event, "page_updated");
    assert!(event.page.is_some());
    assert!(event.issue.is_none());

    let page = event.page.unwrap();
    assert_eq!(page.id, "98765");
    assert_eq!(page.space_key, Some("DEV".to_string()));
    assert_eq!(page.space.unwrap().key, "DEV");
}

#[tokio::test]
async fn test_webhook_event_deserialization_delete_events() {
    let json = serde_json::json!({
        "webhookEvent": "jira:issue_deleted",
        "issue": {
            "id": "10002",
            "key": "PROJ-99",
            "fields": {
                "project": {
                    "key": "PROJ"
                }
            }
        }
    });
    let event: AtlassianWebhookEvent = serde_json::from_value(json).unwrap();
    assert_eq!(event.webhook_event, "jira:issue_deleted");

    let json = serde_json::json!({
        "webhookEvent": "page_trashed",
        "page": {
            "id": "54321",
            "spaceKey": "TEAM"
        }
    });
    let event: AtlassianWebhookEvent = serde_json::from_value(json).unwrap();
    assert_eq!(event.webhook_event, "page_trashed");
    assert_eq!(event.page.unwrap().space_key, Some("TEAM".to_string()));
}

// --- Helpers ---

fn make_test_confluence_page() -> ConfluencePage {
    ConfluencePage {
        id: "123456".to_string(),
        status: ConfluencePageStatus::Current,
        title: "Test Page".to_string(),
        space_id: "98765".to_string(),
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
            number: 1,
            minor_edit: false,
            author_id: "user123".to_string(),
        },
        body: None,
        links: ConfluencePageLinks {
            webui: "/spaces/TEST/pages/123456/Test+Page".to_string(),
            editui: String::new(),
            tinyui: String::new(),
        },
    }
}

fn make_test_jira_issue() -> JiraIssue {
    JiraIssue {
        id: "10001".to_string(),
        key: "PROJ-123".to_string(),
        self_url: format!("{}/rest/api/3/issue/10001", TEST_BASE_URL),
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
            created: "2024-01-01T10:00:00.000+0000".to_string(),
            updated: "2024-01-01T10:00:00.000+0000".to_string(),
            labels: None,
            comment: None,
            components: None,
            extra_fields: HashMap::new(),
        },
    }
}
