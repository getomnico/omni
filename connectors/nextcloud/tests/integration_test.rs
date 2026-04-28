//! Integration tests for the Nextcloud connector.
//!
//! These tests validate the end-to-end flow from raw WebDAV XML responses
//! through file listing, filtering, content extraction, markdown generation,
//! and connector event creation — exercising multiple modules working together
//! as they would in a real sync.

use omni_connector_sdk::ConnectorEvent;
use omni_nextcloud_connector::client::parse_multistatus;
use omni_nextcloud_connector::config::NextcloudConfig;
use omni_nextcloud_connector::models::{DavEntry, NextcloudConnectorState};
use omni_nextcloud_connector::sync::build_file_event;
use std::collections::{HashMap, HashSet};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

// ---------------------------------------------------------------------------
// Helper builders
// ---------------------------------------------------------------------------

fn make_config(server_url: &str) -> NextcloudConfig {
    NextcloudConfig {
        server_url: server_url.to_string(),
        base_path: "/".to_string(),
        extension_allowlist: vec![],
        extension_denylist: vec![],
        max_file_size: 0,
        sync_enabled: true,
    }
}

fn sample_file_entry(name: &str, file_id: &str, etag: &str) -> DavEntry {
    DavEntry {
        href: format!("/remote.php/dav/files/alice/{}", name),
        is_collection: false,
        display_name: Some(name.to_string()),
        content_type: Some("text/plain".to_string()),
        content_length: Some(42),
        etag: Some(etag.to_string()),
        last_modified: Some("Thu, 01 Jan 2024 00:00:00 GMT".to_string()),
        creation_date: Some("2024-01-01T00:00:00+00:00".to_string()),
        file_id: Some(file_id.to_string()),
        permissions: Some("RGDNVW".to_string()),
        oc_size: Some(42),
        owner_id: Some("alice".to_string()),
        owner_display_name: Some("Alice Smith".to_string()),
        favorite: false,
    }
}

fn multistatus_xml(entries_xml: &str) -> String {
    format!(
        r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:" xmlns:oc="http://owncloud.org/ns" xmlns:nc="http://nextcloud.org/ns">
  {entries_xml}
</d:multistatus>"#
    )
}

fn response_xml(
    href: &str,
    display_name: &str,
    is_collection: bool,
    file_id: &str,
    etag: &str,
    content_type: &str,
    size: u64,
) -> String {
    let resource_type = if is_collection {
        "<d:resourcetype><d:collection/></d:resourcetype>"
    } else {
        "<d:resourcetype/>"
    };
    format!(
        r#"<d:response>
    <d:href>{href}</d:href>
    <d:propstat>
      <d:prop>
        {resource_type}
        <d:displayname>{display_name}</d:displayname>
        <d:getcontenttype>{content_type}</d:getcontenttype>
        <d:getcontentlength>{size}</d:getcontentlength>
        <d:getetag>"{etag}"</d:getetag>
        <d:getlastmodified>Thu, 01 Jan 2024 00:00:00 GMT</d:getlastmodified>
        <d:creationdate>2024-01-01T00:00:00+00:00</d:creationdate>
        <oc:fileid>{file_id}</oc:fileid>
        <oc:permissions>RGDNVW</oc:permissions>
        <oc:size>{size}</oc:size>
        <oc:owner-id>alice</oc:owner-id>
        <oc:owner-display-name>Alice Smith</oc:owner-display-name>
        <oc:favorite>0</oc:favorite>
        <nc:has-preview>false</nc:has-preview>
      </d:prop>
    </d:propstat>
  </d:response>"#
    )
}

// ---------------------------------------------------------------------------
// WebDAV XML parsing integration tests
// ---------------------------------------------------------------------------

/// Full PROPFIND response with a directory and multiple files, parsed into DavEntry structs.
#[test]
fn test_parse_full_propfind_response() {
    let xml = multistatus_xml(&format!(
        "{}\n{}\n{}\n{}",
        response_xml(
            "/remote.php/dav/files/alice/",
            "alice",
            true,
            "1",
            "root-etag",
            "",
            0,
        ),
        response_xml(
            "/remote.php/dav/files/alice/report.pdf",
            "report.pdf",
            false,
            "10",
            "etag-10",
            "application/pdf",
            51200,
        ),
        response_xml(
            "/remote.php/dav/files/alice/notes.md",
            "notes.md",
            false,
            "11",
            "etag-11",
            "text/markdown",
            128,
        ),
        response_xml(
            "/remote.php/dav/files/alice/Photos/",
            "Photos",
            true,
            "20",
            "etag-20",
            "",
            0,
        ),
    ));

    // Actually parse the XML using the real parser
    let entries = parse_multistatus(&xml).unwrap();
    assert_eq!(entries.len(), 4);

    // First entry: root collection
    assert!(entries[0].is_collection);
    assert_eq!(entries[0].href, "/remote.php/dav/files/alice/");
    assert_eq!(entries[0].file_id.as_deref(), Some("1"));

    // Second entry: PDF file
    assert!(!entries[1].is_collection);
    assert_eq!(entries[1].filename(), "report.pdf");
    assert_eq!(entries[1].file_id.as_deref(), Some("10"));
    assert_eq!(entries[1].content_type.as_deref(), Some("application/pdf"));
    assert_eq!(entries[1].content_length, Some(51200));
    assert_eq!(entries[1].etag.as_deref(), Some("etag-10"));
    assert_eq!(entries[1].owner_id.as_deref(), Some("alice"));
    assert_eq!(
        entries[1].owner_display_name.as_deref(),
        Some("Alice Smith")
    );
    assert_eq!(entries[1].permissions.as_deref(), Some("RGDNVW"));

    // Third entry: markdown file
    assert!(!entries[2].is_collection);
    assert_eq!(entries[2].filename(), "notes.md");
    assert_eq!(entries[2].content_length, Some(128));

    // Fourth entry: Photos directory
    assert!(entries[3].is_collection);
    assert_eq!(entries[3].filename(), "Photos");

    // Verify filtering (skip collections, as the sync loop does)
    let file_entries: Vec<&DavEntry> = entries.iter().filter(|e| !e.is_collection).collect();
    assert_eq!(file_entries.len(), 2);
    assert_eq!(file_entries[0].filename(), "report.pdf");
    assert_eq!(file_entries[1].filename(), "notes.md");
}

/// Verify that `<d:collection></d:collection>` (non-self-closing) is correctly
/// detected as a collection, not just `<d:collection/>`.
#[test]
fn test_parse_non_self_closing_collection_tag() {
    let xml = multistatus_xml(
        r#"<d:response>
    <d:href>/remote.php/dav/files/alice/Docs/</d:href>
    <d:propstat>
      <d:prop>
        <d:resourcetype><d:collection></d:collection></d:resourcetype>
        <d:displayname>Docs</d:displayname>
        <oc:fileid>99</oc:fileid>
      </d:prop>
    </d:propstat>
  </d:response>"#,
    );

    let entries = parse_multistatus(&xml).unwrap();
    assert_eq!(entries.len(), 1);
    assert!(
        entries[0].is_collection,
        "non-self-closing <d:collection></d:collection> must be recognised"
    );
    assert_eq!(entries[0].filename(), "Docs");
}

// ---------------------------------------------------------------------------
// Incremental sync simulation: etag-based change detection
// ---------------------------------------------------------------------------

/// Simulate the core sync logic: etag comparison determines which files are
/// new, changed, or deleted. This runs through the same steps as execute_sync.
#[test]
fn test_incremental_sync_etag_detection() {
    let config = make_config("https://cloud.example.com");

    // Initial state: one known file
    let mut state = NextcloudConnectorState {
        etags: HashMap::from([("42".into(), "etag-v1".into())]),
        known_files: vec!["42".into()],
    };

    // Current listing from server: file 42 has new etag, file 43 is new
    let entries = vec![
        sample_file_entry("report.pdf", "42", "etag-v2"),
        sample_file_entry("notes.md", "43", "etag-v1"),
    ];

    // Filter (no collections)
    let file_entries: Vec<&DavEntry> = entries
        .iter()
        .filter(|e| !e.is_collection && config.should_index_file(&e.filename()))
        .collect();
    assert_eq!(file_entries.len(), 2);

    // Build current file keys
    let current_keys: HashSet<String> = file_entries.iter().map(|e| e.file_key()).collect();

    // Deletion detection
    let known_set: HashSet<String> = state.known_files.iter().cloned().collect();
    let deleted: Vec<String> = known_set.difference(&current_keys).cloned().collect();
    assert!(
        deleted.is_empty(),
        "File 42 still exists, no deletions expected"
    );

    // Change detection
    let mut changed = Vec::new();
    let mut brand_new = Vec::new();
    for entry in &file_entries {
        let key = entry.file_key();
        if let Some(stored_etag) = state.etags.get(&key) {
            if entry.etag.as_deref() != Some(stored_etag.as_str()) {
                changed.push(key);
            }
        } else {
            brand_new.push(key);
        }
    }

    assert_eq!(changed, vec!["42"]);
    assert_eq!(brand_new, vec!["43"]);

    // Update state after processing
    for entry in &file_entries {
        let key = entry.file_key();
        if let Some(ref etag) = entry.etag {
            state.etags.insert(key.clone(), etag.clone());
        }
    }
    state.known_files = current_keys.into_iter().collect();

    assert_eq!(state.etags.get("42").unwrap(), "etag-v2");
    assert_eq!(state.etags.get("43").unwrap(), "etag-v1");
    assert_eq!(state.known_files.len(), 2);
}

/// Simulate deletion detection: a file that was known is no longer in the listing.
#[test]
fn test_deletion_detection() {
    let state = NextcloudConnectorState {
        etags: HashMap::from([
            ("10".into(), "e1".into()),
            ("20".into(), "e2".into()),
            ("30".into(), "e3".into()),
        ]),
        known_files: vec!["10".into(), "20".into(), "30".into()],
    };

    // Server now only has file 10 and 30 — file 20 was deleted
    let current_keys: HashSet<String> = ["10".into(), "30".into()].into();
    let known_set: HashSet<String> = state.known_files.iter().cloned().collect();
    let deleted: Vec<String> = known_set.difference(&current_keys).cloned().collect();

    assert_eq!(deleted.len(), 1);
    assert_eq!(deleted[0], "20");
}

/// Full sync resets connector state.
#[test]
fn test_full_sync_resets_state() {
    let state = NextcloudConnectorState {
        etags: HashMap::from([("42".into(), "abc".into())]),
        known_files: vec!["42".into()],
    };

    // Simulate "full" sync mode
    let reset_state = NextcloudConnectorState::default();
    assert!(reset_state.etags.is_empty());
    assert!(reset_state.known_files.is_empty());

    // Confirm original wasn't mutated (just checking our logic)
    assert!(!state.etags.is_empty());
}

// ---------------------------------------------------------------------------
// File extension filtering integration
// ---------------------------------------------------------------------------

/// Extension filters interact correctly with the file listing, just like
/// the sync loop filters entries before processing.
#[test]
fn test_extension_filter_integration() {
    let mut config = make_config("https://nc.local");
    config.extension_allowlist = vec!["pdf".into(), "md".into()];
    config.extension_denylist = vec!["tmp".into()];

    let entries = vec![
        sample_file_entry("report.pdf", "1", "e1"),
        sample_file_entry("notes.md", "2", "e2"),
        sample_file_entry("image.png", "3", "e3"),
        sample_file_entry("cache.tmp", "4", "e4"),
        sample_file_entry("README.MD", "5", "e5"), // case-insensitive
    ];

    let filtered: Vec<&DavEntry> = entries
        .iter()
        .filter(|e| !e.is_collection && config.should_index_file(&e.filename()))
        .collect();

    let names: Vec<String> = filtered.iter().map(|e| e.filename()).collect();
    assert_eq!(names, vec!["report.pdf", "notes.md", "README.MD"]);
}

/// Max file size filtering happens in the sync loop. Simulate it.
#[test]
fn test_max_file_size_filter() {
    let mut config = make_config("https://nc.local");
    config.max_file_size = 1000; // 1000 bytes

    let small = DavEntry {
        href: "/remote.php/dav/files/alice/small.txt".into(),
        content_length: Some(500),
        ..Default::default()
    };
    let large = DavEntry {
        href: "/remote.php/dav/files/alice/huge.bin".into(),
        content_length: Some(5000),
        ..Default::default()
    };

    let should_skip = |e: &DavEntry| -> bool {
        let file_size = e.content_length.or(e.oc_size).unwrap_or(0);
        config.max_file_size > 0 && file_size > config.max_file_size
    };

    assert!(!should_skip(&small));
    assert!(should_skip(&large));
}

// ---------------------------------------------------------------------------
// Markdown generation integration
// ---------------------------------------------------------------------------

/// End-to-end markdown: metadata + content combined for indexing.
#[test]
fn test_markdown_generation_end_to_end() {
    let entry = DavEntry {
        href: "/remote.php/dav/files/alice/Documents/quarterly-report.pdf".into(),
        is_collection: false,
        display_name: Some("quarterly-report.pdf".into()),
        content_type: Some("application/pdf".into()),
        content_length: Some(204800),
        etag: Some("deadbeef".into()),
        last_modified: Some("Fri, 15 Mar 2024 14:30:00 GMT".into()),
        creation_date: Some("2024-03-01T09:00:00+00:00".into()),
        file_id: Some("99".into()),
        permissions: Some("RGDNVW".into()),
        oc_size: Some(204800),
        owner_id: Some("alice".into()),
        owner_display_name: Some("Alice Smith".into()),
        favorite: true,
    };

    let content = "Revenue increased by 15% in Q1 2024.";
    let md = entry.to_markdown("alice", "https://cloud.example.com", content);

    // Title
    assert!(md.starts_with("# quarterly-report.pdf\n\n"));
    // Metadata table present and populated
    assert!(md.contains("| Property | Value |"));
    assert!(md.contains("| Path | /Documents/quarterly-report.pdf |"));
    assert!(md.contains("| Content Type | application/pdf |"));
    assert!(md.contains("| Size | 204800 bytes |"));
    assert!(md.contains("| Last Modified | Fri, 15 Mar 2024 14:30:00 GMT |"));
    assert!(md.contains("| Created | 2024-03-01T09:00:00+00:00 |"));
    assert!(md.contains("| Owner | Alice Smith |"));
    assert!(md.contains("| File ID | 99 |"));
    assert!(md.contains("| ETag | deadbeef |"));
    assert!(md.contains("| Permissions | RGDNVW |"));
    assert!(md.contains("| Favorite | Yes |"));
    assert!(md.contains("| URL | https://cloud.example.com/f/99 |"));
    // Content section
    assert!(md.contains("## Content\n\nRevenue increased by 15% in Q1 2024."));
}

/// Markdown with empty content omits the Content section.
#[test]
fn test_markdown_no_content_section_when_empty() {
    let entry = DavEntry {
        href: "/remote.php/dav/files/alice/image.png".into(),
        display_name: Some("image.png".into()),
        content_type: Some("image/png".into()),
        file_id: Some("55".into()),
        ..Default::default()
    };
    let md = entry.to_markdown("alice", "https://nc.local", "");
    assert!(!md.contains("## Content"));
}

// ---------------------------------------------------------------------------
// Document ID stability
// ---------------------------------------------------------------------------

/// Document IDs are deterministic and stable across calls.
#[test]
fn test_document_id_stability() {
    let entry = sample_file_entry("report.pdf", "42", "e1");

    let id1 = entry.document_id("source-abc");
    let id2 = entry.document_id("source-abc");
    assert_eq!(id1, id2, "Same entry + same source = same document ID");

    // Different source → different ID
    let id3 = entry.document_id("source-other");
    assert_ne!(id1, id3);

    // Entry without file_id falls back to href-based key
    let no_id = DavEntry {
        href: "/remote.php/dav/files/alice/file.txt".into(),
        file_id: None,
        ..Default::default()
    };
    let id4 = no_id.document_id("s1");
    let id5 = no_id.document_id("s1");
    assert_eq!(id4, id5);
    assert!(id4.contains("nextcloud:s1:"));
}

// ---------------------------------------------------------------------------
// ConnectorEvent generation integration
// ---------------------------------------------------------------------------

/// build_file_event produces DocumentCreated with correct fields.
#[test]
fn test_build_file_event_created_integration() {
    let entry = DavEntry {
        href: "/remote.php/dav/files/alice/Documents/spec.docx".into(),
        is_collection: false,
        display_name: Some("spec.docx".into()),
        content_type: Some(
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document".into(),
        ),
        content_length: Some(98304),
        etag: Some("etag-999".into()),
        file_id: Some("100".into()),
        owner_id: Some("alice".into()),
        owner_display_name: Some("Alice Smith".into()),
        permissions: Some("RGDNVW".into()),
        favorite: true,
        ..Default::default()
    };

    let event = build_file_event(
        &entry,
        "alice",
        "https://cloud.example.com",
        "run-123",
        "src-456",
        "content-789",
        Some("alice@example.com"),
        false, // new file
    );

    match event {
        ConnectorEvent::DocumentCreated {
            sync_run_id,
            source_id,
            document_id,
            content_id,
            metadata,
            permissions,
            attributes,
        } => {
            assert_eq!(sync_run_id, "run-123");
            assert_eq!(source_id, "src-456");
            assert!(document_id.starts_with("nextcloud:src-456:"));
            assert_eq!(content_id, "content-789");
            assert_eq!(metadata.title.as_deref(), Some("spec.docx"));
            assert_eq!(metadata.author.as_deref(), Some("Alice Smith"));
            assert_eq!(metadata.size.as_deref(), Some("98304"));
            assert!(metadata.url.as_ref().unwrap().contains("/f/100"));
            assert_eq!(metadata.path.as_deref(), Some("/Documents/spec.docx"));
            assert_eq!(
                metadata.content_type.as_deref(),
                Some("application/vnd.openxmlformats-officedocument.wordprocessingml.document")
            );
            assert!(!permissions.public);
            assert_eq!(permissions.users, vec!["alice@example.com"]);

            let attrs = attributes.unwrap();
            assert_eq!(
                attrs.get("file_extension").unwrap(),
                &serde_json::json!("docx")
            );

            // Extra metadata captured
            let extra = metadata.extra.unwrap();
            assert_eq!(extra.get("file_id").unwrap(), &serde_json::json!("100"));
            assert_eq!(extra.get("favorite").unwrap(), &serde_json::json!(true));
            assert_eq!(extra.get("owner_id").unwrap(), &serde_json::json!("alice"));
        }
        other => panic!("Expected DocumentCreated, got {:?}", other),
    }
}

/// build_file_event produces DocumentUpdated when is_update=true.
#[test]
fn test_build_file_event_updated_integration() {
    let entry = sample_file_entry("notes.txt", "50", "e1");

    let event = build_file_event(
        &entry,
        "alice",
        "https://nc.local",
        "run-1",
        "src-1",
        "cnt-1",
        None, // no user email
        true, // update
    );

    match event {
        ConnectorEvent::DocumentUpdated {
            permissions,
            metadata,
            ..
        } => {
            assert!(
                permissions.unwrap().users.is_empty(),
                "No user email → empty users"
            );
            assert_eq!(metadata.title.as_deref(), Some("notes.txt"));
        }
        other => panic!("Expected DocumentUpdated, got {:?}", other),
    }
}

/// Event for a file without file_id still generates a valid document ID.
#[test]
fn test_build_file_event_no_file_id() {
    let entry = DavEntry {
        href: "/remote.php/dav/files/bob/readme.md".into(),
        display_name: Some("readme.md".into()),
        ..Default::default()
    };

    let event = build_file_event(
        &entry,
        "bob",
        "https://nc.local",
        "run-1",
        "src-1",
        "cnt-1",
        None,
        false,
    );

    match event {
        ConnectorEvent::DocumentCreated {
            document_id,
            metadata,
            ..
        } => {
            // Document ID derived from href
            assert!(document_id.starts_with("nextcloud:src-1:"));
            assert!(!document_id.contains("None"));
            // URL falls back to href-based URL (no file_id)
            assert!(metadata
                .url
                .as_ref()
                .unwrap()
                .contains("/remote.php/dav/files/bob/readme.md"));
        }
        _ => panic!("Expected DocumentCreated"),
    }
}

// ---------------------------------------------------------------------------
// Connector state serialization round-trip
// ---------------------------------------------------------------------------

/// State survives serialization to serde_json::Value and back, as it would
/// when persisted via connector-manager.
#[test]
fn test_connector_state_full_round_trip() {
    let mut state = NextcloudConnectorState::default();
    state.etags.insert("10".into(), "abc".into());
    state.etags.insert("20".into(), "def".into());
    state.known_files = vec!["10".into(), "20".into()];

    let json_val = state.to_json();
    // Simulate what connector-manager stores and returns
    let serialized = serde_json::to_string(&json_val).unwrap();
    let deserialized: serde_json::Value = serde_json::from_str(&serialized).unwrap();
    let restored = NextcloudConnectorState::from_connector_state(&Some(deserialized));

    assert_eq!(restored.etags.len(), 2);
    assert_eq!(restored.etags.get("10").unwrap(), "abc");
    assert_eq!(restored.etags.get("20").unwrap(), "def");
    assert_eq!(restored.known_files.len(), 2);
    assert!(restored.known_files.contains(&"10".to_string()));
    assert!(restored.known_files.contains(&"20".to_string()));
}

/// Corrupted/invalid state gracefully falls back to empty default.
#[test]
fn test_connector_state_from_invalid_json() {
    let invalid = Some(serde_json::json!("not a valid state object"));
    let state = NextcloudConnectorState::from_connector_state(&invalid);
    assert!(state.etags.is_empty());
    assert!(state.known_files.is_empty());
}

// ---------------------------------------------------------------------------
// Config ↔ WebDAV URL integration
// ---------------------------------------------------------------------------

/// Config serialization and WebDAV URL building work end-to-end.
#[test]
fn test_config_serialization_and_url() {
    let config_json = serde_json::json!({
        "server_url": "https://my-cloud.example.com/",
        "base_path": "/Work/Projects",
        "extension_allowlist": ["pdf", "docx"],
        "extension_denylist": ["tmp"],
        "max_file_size": 10485760,
        "sync_enabled": true,
    });

    let config = NextcloudConfig::from_source_config(&config_json).unwrap();
    assert_eq!(
        config.webdav_base_url("testuser"),
        "https://my-cloud.example.com/remote.php/dav/files/testuser/Work/Projects"
    );
    assert!(config.should_index_file("report.pdf"));
    assert!(config.should_index_file("spec.DOCX"));
    assert!(!config.should_index_file("image.png"));
    assert!(!config.should_index_file("scratch.tmp"));
    assert_eq!(config.max_file_size, 10485760);
}

// ---------------------------------------------------------------------------
// WebDAV client integration (wiremock)
// ---------------------------------------------------------------------------

/// The NextcloudClient correctly issues PROPFIND and parses the XML response
/// from a real (mocked) HTTP server.
#[tokio::test]
async fn test_list_files_via_wiremock() {
    let server = MockServer::start().await;

    let xml_body = multistatus_xml(&format!(
        "{}\n{}",
        response_xml(
            "/remote.php/dav/files/alice/",
            "alice",
            true,
            "1",
            "root",
            "",
            0,
        ),
        response_xml(
            "/remote.php/dav/files/alice/hello.txt",
            "hello.txt",
            false,
            "42",
            "etag-42",
            "text/plain",
            13,
        ),
    ));

    Mock::given(method("PROPFIND"))
        .and(path("/remote.php/dav/files/alice/"))
        .and(header("Depth", "infinity"))
        .respond_with(ResponseTemplate::new(207).set_body_string(xml_body.clone()))
        .mount(&server)
        .await;

    let client = omni_nextcloud_connector::client::NextcloudClient::new("alice", "secret");
    let url = format!("{}/remote.php/dav/files/alice/", server.uri());
    let entries = client.list_files(&url).await.unwrap();

    // Should have the root collection + file
    assert!(entries.len() >= 1);
    let files: Vec<&DavEntry> = entries.iter().filter(|e| !e.is_collection).collect();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].filename(), "hello.txt");
    assert_eq!(files[0].file_id.as_deref(), Some("42"));
    assert_eq!(files[0].etag.as_deref(), Some("etag-42"));
}

/// The NextcloudClient falls back to recursive Depth:1 when infinity is rejected.
#[tokio::test]
async fn test_list_files_fallback_to_depth_1() {
    let server = MockServer::start().await;

    // Depth: infinity returns 403 (forbidden)
    Mock::given(method("PROPFIND"))
        .and(path("/remote.php/dav/files/alice/"))
        .and(header("Depth", "infinity"))
        .respond_with(ResponseTemplate::new(403))
        .mount(&server)
        .await;

    // Depth: 1 on root returns the root + one file
    let root_xml = multistatus_xml(&format!(
        "{}\n{}",
        response_xml(
            "/remote.php/dav/files/alice/",
            "alice",
            true,
            "1",
            "root",
            "",
            0,
        ),
        response_xml(
            "/remote.php/dav/files/alice/readme.md",
            "readme.md",
            false,
            "50",
            "etag-50",
            "text/markdown",
            256,
        ),
    ));

    Mock::given(method("PROPFIND"))
        .and(path("/remote.php/dav/files/alice/"))
        .and(header("Depth", "1"))
        .respond_with(ResponseTemplate::new(207).set_body_string(root_xml))
        .mount(&server)
        .await;

    let client = omni_nextcloud_connector::client::NextcloudClient::new("alice", "secret");
    let url = format!("{}/remote.php/dav/files/alice/", server.uri());
    let entries = client.list_files(&url).await.unwrap();

    let files: Vec<&DavEntry> = entries.iter().filter(|e| !e.is_collection).collect();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].filename(), "readme.md");
}

/// Download a file via wiremock.
#[tokio::test]
async fn test_download_file_via_wiremock() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/remote.php/dav/files/alice/hello.txt"))
        .respond_with(ResponseTemplate::new(200).set_body_string("Hello, Nextcloud!"))
        .mount(&server)
        .await;

    let client = omni_nextcloud_connector::client::NextcloudClient::new("alice", "pass");
    let url = format!("{}/remote.php/dav/files/alice/hello.txt", server.uri());
    let data = client.download_file(&url).await.unwrap();
    assert_eq!(String::from_utf8(data).unwrap(), "Hello, Nextcloud!");
}

/// Validate credentials: success (207) → true.
#[tokio::test]
async fn test_validate_credentials_success() {
    let server = MockServer::start().await;

    Mock::given(method("PROPFIND"))
        .and(path("/remote.php/dav/files/alice/"))
        .and(header("Depth", "0"))
        .respond_with(
            ResponseTemplate::new(207).set_body_string(multistatus_xml(&response_xml(
                "/remote.php/dav/files/alice/",
                "alice",
                true,
                "1",
                "root",
                "",
                0,
            ))),
        )
        .mount(&server)
        .await;

    let client = omni_nextcloud_connector::client::NextcloudClient::new("alice", "pass");
    let url = format!("{}/remote.php/dav/files/alice/", server.uri());
    assert!(client.validate_credentials(&url).await.unwrap());
}

/// Validate credentials: 401 → false.
#[tokio::test]
async fn test_validate_credentials_unauthorized() {
    let server = MockServer::start().await;

    Mock::given(method("PROPFIND"))
        .and(path("/remote.php/dav/files/alice/"))
        .and(header("Depth", "0"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;

    let client = omni_nextcloud_connector::client::NextcloudClient::new("alice", "wrong");
    let url = format!("{}/remote.php/dav/files/alice/", server.uri());
    assert!(!client.validate_credentials(&url).await.unwrap());
}

// ---------------------------------------------------------------------------
// Relative path and URL encoding edge cases
// ---------------------------------------------------------------------------

/// Files with special characters in names produce correct paths and document IDs.
#[test]
fn test_special_characters_in_filenames() {
    let entry = DavEntry {
        href: "/remote.php/dav/files/alice/My%20Documents/Quarterly%20Report%20(Q1).pdf".into(),
        display_name: Some("Quarterly Report (Q1).pdf".into()),
        file_id: Some("77".into()),
        ..Default::default()
    };

    assert_eq!(entry.filename(), "Quarterly Report (Q1).pdf");
    assert_eq!(
        entry.relative_path("alice"),
        "/My Documents/Quarterly Report (Q1).pdf"
    );

    let doc_id = entry.document_id("s1");
    assert_eq!(doc_id, "nextcloud:s1:77"); // file_id is used, not encoded href
}

/// File _without_ display_name decodes the href to get filename.
#[test]
fn test_filename_from_encoded_href() {
    let entry = DavEntry {
        href: "/remote.php/dav/files/alice/R%C3%A9sum%C3%A9.pdf".into(),
        display_name: None,
        ..Default::default()
    };

    assert_eq!(entry.filename(), "Résumé.pdf");
}
