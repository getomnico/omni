mod common;

use anyhow::Result;
use chrono::Utc;
use omni_web_connector::models::{PageSyncState, WebPage};
use omni_web_connector::sync::SyncState;
use std::collections::HashSet;

use common::WebConnectorTestFixture;

#[tokio::test]
async fn test_page_sync_state_set_and_get() -> Result<()> {
    let fixture = WebConnectorTestFixture::new().await?;
    let sync_state = SyncState::new(fixture.redis_client());

    let source_id = "test_web_source";
    let url = "https://example.com/page1";

    // Initially, no state should exist
    assert!(sync_state
        .get_page_sync_state(source_id, url)
        .await?
        .is_none());

    // Create and set the state
    let page_state = PageSyncState {
        last_modified: Some("Mon, 01 Jan 2024 00:00:00 GMT".to_string()),
        etag: Some("abc123".to_string()),
        content_hash: "hash123".to_string(),
        last_synced: Utc::now(),
    };

    sync_state
        .set_page_sync_state(source_id, url, &page_state)
        .await?;

    // State should now exist
    let retrieved = sync_state
        .get_page_sync_state(source_id, url)
        .await?
        .expect("State should exist");

    assert_eq!(retrieved.content_hash, "hash123");
    assert_eq!(retrieved.etag, Some("abc123".to_string()));

    // Clean up
    sync_state.delete_page_sync_state(source_id, url).await?;

    Ok(())
}

#[tokio::test]
async fn test_page_sync_state_delete() -> Result<()> {
    let fixture = WebConnectorTestFixture::new().await?;
    let sync_state = SyncState::new(fixture.redis_client());

    let source_id = "test_source_delete";
    let url = "https://example.com/delete-test";

    let page_state = PageSyncState {
        last_modified: None,
        etag: None,
        content_hash: "hash456".to_string(),
        last_synced: Utc::now(),
    };

    // Set the state
    sync_state
        .set_page_sync_state(source_id, url, &page_state)
        .await?;

    // Verify it exists
    assert!(sync_state
        .get_page_sync_state(source_id, url)
        .await?
        .is_some());

    // Delete it
    sync_state.delete_page_sync_state(source_id, url).await?;

    // Verify it's gone
    assert!(sync_state
        .get_page_sync_state(source_id, url)
        .await?
        .is_none());

    Ok(())
}

#[tokio::test]
async fn test_url_set_operations() -> Result<()> {
    let fixture = WebConnectorTestFixture::new().await?;
    let sync_state = SyncState::new(fixture.redis_client());

    let source_id = "test_source_urls";
    let urls = vec![
        "https://example.com/page1",
        "https://example.com/page2",
        "https://example.com/page3",
    ];

    // Add URLs to set
    for url in &urls {
        sync_state.add_url_to_set(source_id, url).await?;
    }

    // Get all synced URLs (returns hashes)
    let synced_urls = sync_state.get_all_synced_urls(source_id).await?;
    assert_eq!(synced_urls.len(), 3);

    // Compute expected hashes
    let expected_hashes: HashSet<String> = urls
        .iter()
        .map(|url| format!("{:x}", md5::compute(url)))
        .collect();
    assert_eq!(synced_urls, expected_hashes);

    Ok(())
}

#[tokio::test]
async fn test_remove_url_from_set() -> Result<()> {
    let fixture = WebConnectorTestFixture::new().await?;
    let sync_state = SyncState::new(fixture.redis_client());

    let source_id = "test_source_remove";
    let url1 = "https://example.com/keep";
    let url2 = "https://example.com/remove";

    // Add URLs
    sync_state.add_url_to_set(source_id, url1).await?;
    sync_state.add_url_to_set(source_id, url2).await?;

    // Verify both exist
    let urls = sync_state.get_all_synced_urls(source_id).await?;
    assert_eq!(urls.len(), 2);

    // Remove one URL (by hash)
    let url2_hash = format!("{:x}", md5::compute(url2));
    sync_state
        .remove_url_from_set(source_id, &url2_hash)
        .await?;

    // Verify only one remains
    let urls = sync_state.get_all_synced_urls(source_id).await?;
    assert_eq!(urls.len(), 1);
    assert!(urls.contains(&format!("{:x}", md5::compute(url1))));

    Ok(())
}

#[tokio::test]
async fn test_page_state_isolation_between_sources() -> Result<()> {
    let fixture = WebConnectorTestFixture::new().await?;
    let sync_state = SyncState::new(fixture.redis_client());

    let source_a = "source_a";
    let source_b = "source_b";
    let url = "https://example.com/shared-url";

    let state_a = PageSyncState {
        last_modified: None,
        etag: None,
        content_hash: "hash_source_a".to_string(),
        last_synced: Utc::now(),
    };

    let state_b = PageSyncState {
        last_modified: None,
        etag: None,
        content_hash: "hash_source_b".to_string(),
        last_synced: Utc::now(),
    };

    // Set state for same URL in different sources
    sync_state
        .set_page_sync_state(source_a, url, &state_a)
        .await?;
    sync_state
        .set_page_sync_state(source_b, url, &state_b)
        .await?;

    // Verify isolation
    let retrieved_a = sync_state
        .get_page_sync_state(source_a, url)
        .await?
        .expect("Source A state should exist");
    let retrieved_b = sync_state
        .get_page_sync_state(source_b, url)
        .await?
        .expect("Source B state should exist");

    assert_eq!(retrieved_a.content_hash, "hash_source_a");
    assert_eq!(retrieved_b.content_hash, "hash_source_b");

    Ok(())
}

/// Pure unit test for change detection logic
#[test]
fn test_change_detection_logic() {
    struct TestCase {
        old_etag: Option<&'static str>,
        new_etag: Option<&'static str>,
        old_modified: Option<&'static str>,
        new_modified: Option<&'static str>,
        old_hash: &'static str,
        new_hash: &'static str,
        expected_changed: bool,
        description: &'static str,
    }

    let test_cases = vec![
        TestCase {
            old_etag: Some("etag1"),
            new_etag: Some("etag2"),
            old_modified: None,
            new_modified: None,
            old_hash: "hash",
            new_hash: "hash",
            expected_changed: true,
            description: "Etag changed should trigger update",
        },
        TestCase {
            old_etag: Some("etag1"),
            new_etag: Some("etag1"),
            old_modified: None,
            new_modified: None,
            old_hash: "hash",
            new_hash: "hash",
            expected_changed: false,
            description: "Same etag should skip",
        },
        TestCase {
            old_etag: None,
            new_etag: None,
            old_modified: Some("Mon, 01 Jan 2024"),
            new_modified: Some("Tue, 02 Jan 2024"),
            old_hash: "hash",
            new_hash: "hash",
            expected_changed: true,
            description: "Modified time changed should trigger update",
        },
        TestCase {
            old_etag: None,
            new_etag: None,
            old_modified: None,
            new_modified: None,
            old_hash: "hash1",
            new_hash: "hash2",
            expected_changed: true,
            description: "Content hash changed should trigger update",
        },
        TestCase {
            old_etag: None,
            new_etag: None,
            old_modified: None,
            new_modified: None,
            old_hash: "hash",
            new_hash: "hash",
            expected_changed: false,
            description: "No changes should skip",
        },
    ];

    for tc in test_cases {
        let state = PageSyncState {
            last_modified: tc.old_modified.map(String::from),
            etag: tc.old_etag.map(String::from),
            content_hash: tc.old_hash.to_string(),
            last_synced: Utc::now(),
        };

        let page = WebPage {
            url: "https://example.com".to_string(),
            title: None,
            description: None,
            content: "content".to_string(),
            content_hash: tc.new_hash.to_string(),
            last_modified: tc.new_modified.map(String::from),
            etag: tc.new_etag.map(String::from),
            word_count: 1,
        };

        assert_eq!(
            state.has_changed(&page),
            tc.expected_changed,
            "Failed: {}",
            tc.description
        );
    }
}
