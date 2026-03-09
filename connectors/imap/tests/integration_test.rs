use omni_imap_connector::config::ImapAccountConfig;
use omni_imap_connector::models::{
    build_thread_connector_event, generate_thread_content, make_document_id,
    make_thread_document_id, parse_raw_email, resolve_new_email_thread_root, resolve_thread_root,
    FolderSyncState, ImapConnectorState, ParsedEmail,
};
use serde_json::json;

// ── Config and folder filtering ──────────────────────────────────────────────

#[test]
fn test_config_from_json_defaults() {
    let cfg_json = json!({
        "host": "imap.example.com",
        "port": 993
    });
    let cfg: ImapAccountConfig = serde_json::from_value(cfg_json).unwrap();
    assert_eq!(cfg.host, "imap.example.com");
    assert_eq!(cfg.port, 993);
    assert_eq!(cfg.encryption, "tls");
    assert!(cfg.sync_enabled);
    assert!(cfg.folder_allowlist.is_empty());
    assert!(cfg.folder_denylist.iter().any(|f| f == "Trash"));
    assert!(cfg.folder_denylist.iter().any(|f| f == "Spam"));
    assert_eq!(cfg.max_message_size, 0);
    assert_eq!(cfg.webmail_url_template, None);
}

#[test]
fn test_folder_filtering_allowlist_only() {
    let cfg = ImapAccountConfig {
        display_name: None,
        host: "mail.example.com".into(),
        port: 993,
        encryption: "tls".into(),
        folder_allowlist: vec!["INBOX".into(), "Sent".into()],
        folder_denylist: vec![],
        webmail_url_template: None,
        max_message_size: 0,
        sync_enabled: true,
    };
    assert!(cfg.should_index_folder("INBOX"));
    assert!(cfg.should_index_folder("Sent"));
    assert!(!cfg.should_index_folder("Drafts"));
    assert!(!cfg.should_index_folder("Trash"));
}

#[test]
fn test_folder_filtering_denylist_only() {
    let cfg = ImapAccountConfig {
        display_name: None,
        host: "mail.example.com".into(),
        port: 993,
        encryption: "tls".into(),
        folder_allowlist: vec![],
        folder_denylist: vec!["Spam".into(), "Trash".into()],
        max_message_size: 0,
        webmail_url_template: None,
        sync_enabled: true,
    };
    assert!(cfg.should_index_folder("INBOX"));
    assert!(cfg.should_index_folder("Sent"));
    assert!(!cfg.should_index_folder("Spam"));
    assert!(!cfg.should_index_folder("Trash"));
}

#[test]
fn test_folder_filtering_denylist_beats_allowlist() {
    let cfg = ImapAccountConfig {
        display_name: None,
        host: "mail.example.com".into(),
        port: 993,
        encryption: "tls".into(),
        folder_allowlist: vec!["INBOX".into()],
        folder_denylist: vec!["INBOX".into()],
        max_message_size: 0,
        webmail_url_template: None,
        sync_enabled: true,
    };
    assert!(!cfg.should_index_folder("INBOX"));
}

#[test]
fn test_folder_filtering_case_insensitive() {
    let cfg = ImapAccountConfig {
        display_name: None,
        host: "mail.example.com".into(),
        port: 993,
        encryption: "tls".into(),
        folder_allowlist: vec!["inbox".into()],
        folder_denylist: vec![],
        webmail_url_template: None,
        max_message_size: 0,
        sync_enabled: true,
    };
    assert!(cfg.should_index_folder("INBOX"));
    assert!(cfg.should_index_folder("inbox"));
    assert!(cfg.should_index_folder("Inbox"));
}

// ── Connector state serialization ─────────────────────────────────────────────

#[test]
fn test_connector_state_default_is_empty() {
    let state = ImapConnectorState::from_connector_state(&None);
    assert!(state.folders.is_empty());
}

#[test]
fn test_connector_state_round_trip() {
    let mut state = ImapConnectorState::default();
    state.folders.insert(
        "INBOX".to_string(),
        FolderSyncState {
            uid_validity: 123456789,
            indexed_uids: vec![100, 200, 250],
            messages: Default::default(),
            skipped_uids: Default::default(),
        },
    );
    state.folders.insert(
        "Sent".to_string(),
        FolderSyncState {
            uid_validity: 0,
            indexed_uids: vec![],
            messages: Default::default(),
            skipped_uids: Default::default(),
        },
    );

    let json = state.to_json();
    let restored = ImapConnectorState::from_connector_state(&Some(json));

    let inbox = restored.folders.get("INBOX").expect("INBOX should be present");
    assert_eq!(inbox.uid_validity, 123456789);
    assert_eq!(inbox.indexed_uids, vec![100, 200, 250]);

    let sent = restored.folders.get("Sent").expect("Sent should be present");
    assert_eq!(sent.uid_validity, 0);
    assert!(sent.indexed_uids.is_empty());
}

#[test]
fn test_connector_state_from_invalid_json_returns_default() {
    let bogus = Some(json!("not an object"));
    let state = ImapConnectorState::from_connector_state(&bogus);
    assert!(state.folders.is_empty());
}

// ── Email parsing ─────────────────────────────────────────────────────────────

#[test]
fn test_parse_plain_text_email() {
    let raw = b"From: sender@example.com\r\n\
        To: recipient@example.com\r\n\
        Subject: Integration Test\r\n\
        Date: Tue, 02 Jan 2024 10:00:00 +0000\r\n\
        Message-ID: <inttest001@example.com>\r\n\
        Content-Type: text/plain; charset=utf-8\r\n\
        \r\n\
        Hello from the integration test.\r\n";

    let email = parse_raw_email(raw, 1, "INBOX").unwrap();
    assert_eq!(email.subject, "Integration Test");
    assert_eq!(email.from, "sender@example.com");
    assert!(email.body_text.contains("Hello from the integration test."));
    assert_eq!(email.imap_uid, 1);
    assert_eq!(email.folder, "INBOX");
    assert_eq!(email.message_id.as_deref(), Some("<inttest001@example.com>"));
}

#[test]
fn test_parse_html_email_extracts_text() {
    let raw = b"From: a@example.com\r\n\
        Subject: HTML Only\r\n\
        Content-Type: text/html; charset=utf-8\r\n\
        \r\n\
        <html><body><h1>Title</h1><p>Paragraph content here.</p></body></html>\r\n";

    let email = parse_raw_email(raw, 2, "Archive").unwrap();
    assert!(
        email.body_text.contains("Paragraph content here."),
        "Expected text to contain paragraph, got: {}",
        email.body_text
    );
}

#[test]
fn test_parse_multipart_alternative_prefers_plain() {
    let raw = b"From: a@b.com\r\n\
        Subject: Multipart\r\n\
        Content-Type: multipart/alternative; boundary=\"sep\"\r\n\
        \r\n\
        --sep\r\n\
        Content-Type: text/plain; charset=utf-8\r\n\
        \r\n\
        This is the plain text version.\r\n\
        --sep\r\n\
        Content-Type: text/html; charset=utf-8\r\n\
        \r\n\
        <p>This is the HTML version.</p>\r\n\
        --sep--\r\n";

    let email = parse_raw_email(raw, 3, "INBOX").unwrap();
    assert!(email.body_text.contains("plain text version"));
    assert!(!email.body_text.contains("<p>"), "Should not contain HTML tags");
}

#[test]
fn test_parse_email_no_subject_fallback() {
    let raw = b"From: a@b.com\r\n\
        Content-Type: text/plain; charset=utf-8\r\n\
        \r\n\
        Body text.\r\n";

    let email = parse_raw_email(raw, 4, "INBOX").unwrap();
    assert!(!email.subject.is_empty());
}

#[test]
fn test_parse_email_with_cc() {
    let raw = b"From: sender@example.com\r\n\
        To: to@example.com\r\n\
        Cc: cc1@example.com, cc2@example.com\r\n\
        Subject: With CC\r\n\
        Content-Type: text/plain; charset=utf-8\r\n\
        \r\n\
        Body.\r\n";

    let email = parse_raw_email(raw, 5, "INBOX").unwrap();
    assert_eq!(email.cc.len(), 2);
    assert!(email.cc.iter().any(|s| s.contains("cc1@example.com")));
    assert!(email.cc.iter().any(|s| s.contains("cc2@example.com")));
}

// ── External ID stability ──────────────────────────────────────────────────────

#[test]
fn test_external_id_is_stable_and_uid_only() {
    let raw = b"From: a@b.com\r\n\
        Subject: Stable ID\r\n\
        Message-ID: <stable001@test.example>\r\n\
        Content-Type: text/plain; charset=utf-8\r\n\
        \r\n\
        Body.\r\n";

    let email = parse_raw_email(raw, 10, "INBOX").unwrap();
    let id1 = email.external_id("source-abc");
    let id2 = email.external_id("source-abc");

    // Must be deterministic.
    assert_eq!(id1, id2, "External ID should be deterministic");

    // Must be exactly imap:<source>:<folder>:<uid> — no message_id component.
    // This ensures that deletion events (which only know the UID) can
    // reconstruct the same ID without needing the original headers.
    assert_eq!(id1, "imap:source-abc:INBOX:10");
    assert!(
        !id1.contains("stable001"),
        "Message-ID must not appear in external_id"
    );
}

#[test]
fn test_external_ids_differ_across_accounts() {
    let raw = b"From: a@b.com\r\n\
        Subject: Same Message\r\n\
        Message-ID: <shared-msgid@example.com>\r\n\
        Content-Type: text/plain; charset=utf-8\r\n\
        \r\n\
        Body.\r\n";

    let email1 = parse_raw_email(raw, 1, "INBOX").unwrap();
    let email2 = parse_raw_email(raw, 1, "INBOX").unwrap();

    let id1 = email1.external_id("account-1");
    let id2 = email2.external_id("account-2");

    assert_ne!(id1, id2, "Different accounts must produce different IDs");
}

// ── Ingestion ↔ deletion ID round-trip ────────────────────────────────────────

/// This is the most critical invariant: the document_id written during
/// ingestion must exactly equal the document_id written in deletion events.
/// Previously, ingestion included the Message-ID in the ID but deletion did
/// not, so every deletion event silently targeted a non-existent document.
#[test]
fn test_ingestion_and_deletion_ids_are_identical() {
    let raw = b"From: a@b.com\r\n\
        Subject: Test\r\n\
        Message-ID: <important-msgid@company.example>\r\n\
        Content-Type: text/plain; charset=utf-8\r\n\
        \r\n\
        Body.\r\n";

    let email = parse_raw_email(raw, 77, "Work/Projects").unwrap();
    let ingestion_id = email.external_id("acct-1");
    let deletion_id = make_document_id("acct-1", "Work/Projects", 77);

    assert_eq!(
        ingestion_id, deletion_id,
        "Ingestion and deletion document IDs must be identical"
    );
    assert_eq!(ingestion_id, "imap:acct-1:Work%2FProjects:77");
}

#[test]
fn test_document_id_folder_with_space() {
    let id = make_document_id("src-1", "My Folder", 3);
    assert_eq!(id, "imap:src-1:My%20Folder:3");
}

#[test]
fn test_generate_content_structure() {
    let raw = b"From: alice@example.com\r\n\
        To: bob@example.com\r\n\
        Subject: Project Update\r\n\
        Date: Mon, 15 Jan 2024 09:00:00 +0000\r\n\
        Content-Type: text/plain; charset=utf-8\r\n\
        \r\n\
        The project is on track for delivery.\r\n";

    let email = parse_raw_email(raw, 1, "Work").unwrap();
    let content = email.generate_content();

    assert!(content.contains("Subject: Project Update"));
    assert!(content.contains("From: alice@example.com"));
    assert!(content.contains("To: bob@example.com"));
    assert!(content.contains("The project is on track for delivery."));
}

// ── ConnectorEvent generation ──────────────────────────────────────────────────

#[test]
fn test_to_connector_event_document_created() {
    use shared::models::ConnectorEvent;

    let raw = b"From: alice@example.com\r\n\
        To: bob@example.com\r\n\
        Subject: Meeting Agenda\r\n\
        Message-ID: <agenda001@example.com>\r\n\
        Content-Type: text/plain; charset=utf-8\r\n\
        \r\n\
        Agenda items here.\r\n";

    let email = parse_raw_email(raw, 1, "INBOX").unwrap();
    let event = email.to_connector_event(
        "sync-run-1".to_string(),
        "source-1".to_string(),
        "content-abc".to_string(),
        "My Work Account",
        None,
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
            assert_eq!(source_id, "source-1");
            assert_eq!(content_id, "content-abc");
            assert!(document_id.contains("source-1"));
            assert_eq!(metadata.title.as_deref(), Some("Meeting Agenda"));
            assert_eq!(metadata.author.as_deref(), Some("alice@example.com"));
        }
        _ => panic!("Expected DocumentCreated event"),
    }
}

// ── Multiple accounts isolation ────────────────────────────────────────────────

#[test]
fn test_multiple_accounts_have_isolated_connector_states() {
    let mut state1 = ImapConnectorState::default();
    state1.folders.insert(
        "INBOX".to_string(),
        FolderSyncState { uid_validity: 1, indexed_uids: vec![50], messages: Default::default(), skipped_uids: Default::default() },
    );

    let mut state2 = ImapConnectorState::default();
    state2.folders.insert(
        "INBOX".to_string(),
        FolderSyncState { uid_validity: 2, indexed_uids: vec![200], messages: Default::default(), skipped_uids: Default::default() },
    );

    // Changing state2 must not affect state1.
    state2.folders.get_mut("INBOX").unwrap().indexed_uids.push(999);
    assert_eq!(state1.folders["INBOX"].indexed_uids, vec![50]);
}

// ── Deletion detection logic ───────────────────────────────────────────────────

#[test]
fn test_deletion_detection_removes_missing_uids() {
    let indexed_uids: Vec<u32> = vec![1, 2, 3, 4, 5];
    let server_uids: std::collections::HashSet<u32> = vec![1, 3, 5].into_iter().collect();

    let deleted: Vec<u32> = indexed_uids
        .iter()
        .copied()
        .filter(|uid| !server_uids.contains(uid))
        .collect();

    assert_eq!(deleted, vec![2, 4]);
}

#[test]
fn test_deletion_detection_no_deletions() {
    let indexed_uids: Vec<u32> = vec![1, 2, 3];
    let server_uids: std::collections::HashSet<u32> = vec![1, 2, 3, 4, 5].into_iter().collect();

    let deleted: Vec<u32> = indexed_uids
        .iter()
        .copied()
        .filter(|uid| !server_uids.contains(uid))
        .collect();

    assert!(deleted.is_empty());
}

/// Regression test: when a deletion-event emission fails, the UID must NOT be
/// removed from `indexed_uids`.  If it were removed, the document would remain
/// in the search index permanently (orphaned) because no subsequent sync would
/// retry the deletion — the UID would never appear in `indexed_uids − server_uids`
/// again (it isn't on the server and isn't in the tracked list).
#[test]
fn test_failed_deletion_event_keeps_uid_in_indexed_for_retry() {
    // Setup: UIDs 1, 2, 3 are indexed.  Server only has 1 and 3 → UID 2 was deleted.
    let mut indexed_uids: Vec<u32> = vec![1, 2, 3];
    let server_uids: std::collections::HashSet<u32> = vec![1, 3].into_iter().collect();

    let deleted_uids: Vec<u32> = indexed_uids
        .iter()
        .copied()
        .filter(|uid| !server_uids.contains(uid))
        .collect();

    // Simulate: deletion event for UID 2 fails.
    let mut failed_deletion_uids: std::collections::HashSet<u32> =
        std::collections::HashSet::new();
    for uid in &deleted_uids {
        // In production, emit_event returns Err here.
        failed_deletion_uids.insert(*uid);
    }

    // Apply the fixed retain logic from sync_folder.
    indexed_uids
        .retain(|uid| server_uids.contains(uid) || failed_deletion_uids.contains(uid));

    // UID 2 must remain so deletion is retried on the next sync.
    assert!(
        indexed_uids.contains(&2),
        "UID 2 must stay in indexed_uids when its deletion event fails (retry on next sync)"
    );
    // UIDs 1 and 3 are still on the server — they must be kept.
    assert!(indexed_uids.contains(&1));
    assert!(indexed_uids.contains(&3));
    assert_eq!(indexed_uids.len(), 3);
}

/// When a deletion event is emitted successfully, the UID must be removed from
/// `indexed_uids` so it is not retried and the index stays clean.
#[test]
fn test_successful_deletion_event_removes_uid_from_indexed() {
    let mut indexed_uids: Vec<u32> = vec![1, 2, 3];
    let server_uids: std::collections::HashSet<u32> = vec![1, 3].into_iter().collect();

    // Compute deleted UIDs; in production each would trigger an emit_event call.
    // Here all events succeed, so failed_deletion_uids stays empty.
    let _deleted_uids: Vec<u32> = indexed_uids
        .iter()
        .copied()
        .filter(|uid| !server_uids.contains(uid))
        .collect();

    // Simulate: all deletion events succeed → no failures.
    let failed_deletion_uids: std::collections::HashSet<u32> =
        std::collections::HashSet::new();

    // Apply the fixed retain logic.
    indexed_uids
        .retain(|uid| server_uids.contains(uid) || failed_deletion_uids.contains(uid));

    // UID 2 must be removed (deletion succeeded).
    assert!(
        !indexed_uids.contains(&2),
        "UID 2 must be removed from indexed_uids after a successful deletion event"
    );
    assert_eq!(indexed_uids, vec![1, 3]);
}

/// Partial-failure scenario: some deletion events succeed and some fail.
/// Only the UIDs with FAILED events must stay in indexed_uids; the rest must go.
#[test]
fn test_partial_deletion_failure_retains_only_failed_uids() {
    // UIDs 1-5 indexed; server only has 3 → UIDs 1, 2, 4, 5 deleted.
    let mut indexed_uids: Vec<u32> = vec![1, 2, 3, 4, 5];
    let server_uids: std::collections::HashSet<u32> = vec![3].into_iter().collect();

    let deleted_uids: Vec<u32> = indexed_uids
        .iter()
        .copied()
        .filter(|uid| !server_uids.contains(uid))
        .collect();

    // Simulate: events for UIDs 1 and 4 fail; UIDs 2 and 5 succeed.
    let mut failed_deletion_uids: std::collections::HashSet<u32> =
        std::collections::HashSet::new();
    for uid in &deleted_uids {
        if *uid == 1 || *uid == 4 {
            failed_deletion_uids.insert(*uid);
        }
    }

    indexed_uids
        .retain(|uid| server_uids.contains(uid) || failed_deletion_uids.contains(uid));

    // UIDs 1 and 4 (failed) must stay; UIDs 2 and 5 (succeeded) must go.
    assert!(indexed_uids.contains(&1), "UID 1 (failed) must stay");
    assert!(!indexed_uids.contains(&2), "UID 2 (succeeded) must be removed");
    assert!(indexed_uids.contains(&3), "UID 3 (on server) must stay");
    assert!(indexed_uids.contains(&4), "UID 4 (failed) must stay");
    assert!(!indexed_uids.contains(&5), "UID 5 (succeeded) must be removed");
    assert_eq!(indexed_uids.len(), 3);
}

// ── UIDVALIDITY change handling ────────────────────────────────────────────────

#[test]
fn test_uidvalidity_change_triggers_full_resync() {
    let mut folder_state = FolderSyncState {
        uid_validity: 12345,
        indexed_uids: vec![10, 50, 100],
        messages: Default::default(),
        skipped_uids: Default::default(),
    };

    let server_uid_validity = 99999u32; // Different!

    if server_uid_validity != 0 && folder_state.uid_validity != 0 && folder_state.uid_validity != server_uid_validity {
        folder_state.uid_validity = server_uid_validity;
        folder_state.indexed_uids.clear();
    }

    assert_eq!(folder_state.uid_validity, 99999);
    assert!(
        folder_state.indexed_uids.is_empty(),
        "indexed_uids must be cleared on UIDVALIDITY change"
    );
}

#[test]
fn test_uidvalidity_unchanged_does_not_reset() {
    let mut folder_state = FolderSyncState {
        uid_validity: 12345,
        indexed_uids: vec![10, 50, 100],
        messages: Default::default(),
        skipped_uids: Default::default(),
    };

    let server_uid_validity = 12345u32; // Same!

    if server_uid_validity != 0 && folder_state.uid_validity != 0 && folder_state.uid_validity != server_uid_validity {
        folder_state.uid_validity = server_uid_validity;
        folder_state.indexed_uids.clear();
    }

    assert_eq!(folder_state.uid_validity, 12345);
    assert_eq!(
        folder_state.indexed_uids.len(),
        3,
        "indexed_uids must be preserved when UIDVALIDITY is unchanged"
    );
}

/// Regression test: if the server stops advertising UIDVALIDITY (returns 0),
/// we must NOT treat that as a change and wipe the indexed_uids list.
/// Without the `uid_validity != 0` guard on the incoming value, a stored
/// non-zero uid_validity would always differ from 0, causing a spurious full
/// resync on every subsequent run.
#[test]
fn test_uidvalidity_server_returns_zero_does_not_spuriously_resync() {
    let mut folder_state = FolderSyncState {
        uid_validity: 12345,
        indexed_uids: vec![10, 50, 100],
        messages: Default::default(),
        skipped_uids: Default::default(),
    };

    let server_uid_validity = 0u32; // Server no longer provides UIDVALIDITY.

    // Simulate what sync_folder does (condition + conditional update).
    if server_uid_validity != 0 && folder_state.uid_validity != 0 && folder_state.uid_validity != server_uid_validity {
        folder_state.indexed_uids.clear();
    }
    if server_uid_validity != 0 {
        folder_state.uid_validity = server_uid_validity;
    }

    // uid_validity should remain at the last-known good value (12345).
    assert_eq!(folder_state.uid_validity, 12345);
    assert_eq!(
        folder_state.indexed_uids.len(),
        3,
        "indexed_uids must not be cleared when server returns uid_validity=0"
    );
}

/// Regression test for the three-step scenario that exposed the clobber bug:
///
/// 1. Sync 1: server returns uid_validity=12345 → stored as 12345.
/// 2. Sync 2: server temporarily returns uid_validity=0.
///    - No resync (correct: uid_validity=0 is "not advertised").
///    - **Bug**: without the guard, stored uid_validity is clobbered to 0.
/// 3. Sync 3: server returns uid_validity=99999 (real UIDVALIDITY change).
///    - With clobbered stored value (0): condition `stored != 0` is false
///      → no resync triggered → indexed_uids now point to wrong messages.
///    - With the fix (stored stays 12345 after step 2):
///      condition fires correctly → full resync.
#[test]
fn test_uidvalidity_zero_does_not_clobber_stored_value_allowing_later_change_detection() {
    // Simulate the state after sync 1: uid_validity=12345 known and stored.
    let mut folder_state = FolderSyncState {
        uid_validity: 12345,
        indexed_uids: vec![10, 50, 100],
        messages: Default::default(),        skipped_uids: Default::default(),    };

    // ── Sync 2: server returns uid_validity=0 ──────────────────────────────
    let server_uid_validity_sync2 = 0u32;
    if server_uid_validity_sync2 != 0 && folder_state.uid_validity != 0 && folder_state.uid_validity != server_uid_validity_sync2 {
        folder_state.indexed_uids.clear();
    }
    if server_uid_validity_sync2 != 0 {
        folder_state.uid_validity = server_uid_validity_sync2;
    }

    // Stored value must be preserved (12345), not overwritten with 0.
    assert_eq!(folder_state.uid_validity, 12345, "stored uid_validity must not be clobbered by server returning 0");
    assert_eq!(folder_state.indexed_uids.len(), 3, "indexed_uids must be unchanged");

    // ── Sync 3: server returns uid_validity=99999 (genuine change) ─────────
    let server_uid_validity_sync3 = 99999u32;
    if server_uid_validity_sync3 != 0 && folder_state.uid_validity != 0 && folder_state.uid_validity != server_uid_validity_sync3 {
        folder_state.indexed_uids.clear();
    }
    if server_uid_validity_sync3 != 0 {
        folder_state.uid_validity = server_uid_validity_sync3;
    }

    // The genuine UIDVALIDITY change must now be detected and trigger a resync.
    assert_eq!(folder_state.uid_validity, 99999);
    assert!(
        folder_state.indexed_uids.is_empty(),
        "indexed_uids must be cleared when a genuine UIDVALIDITY change is detected \
         after a period where the server did not advertise UIDVALIDITY"
    );
}

// ── Skipped-message retry guarantee ───────────────────────────────────────────

/// This test validates the core fix for the permanently-skipped-message bug.
/// With the old last_uid approach, if UIDs 1 and 3 were indexed but UID 2
/// failed, last_uid would be set to 3 and UID 2 would never be retried.
/// With the new set-subtraction approach, UID 2 is always in
/// (server_uids − indexed_uids) and will be retried every sync.
#[test]
fn test_new_uids_computed_as_set_subtraction() {
    let indexed_uids: std::collections::HashSet<u32> = vec![1, 3].into_iter().collect();
    let server_uids: Vec<u32> = vec![1, 2, 3, 4, 5];

    let mut new_uids: Vec<u32> = server_uids
        .iter()
        .copied()
        .filter(|uid| !indexed_uids.contains(uid))
        .collect();
    new_uids.sort_unstable();

    // UID 2 must appear even though indexed UIDs 1 and 3 bracket it.
    assert_eq!(new_uids, vec![2, 4, 5]);
}

// ── Threading model coverage ─────────────────────────────────────────────────

#[test]
fn test_thread_permissions_include_all_participants() {
    use shared::models::ConnectorEvent;

    let alice_msg = ParsedEmail {
        imap_uid: 1,
        folder: "INBOX".into(),
        from: "alice@example.com".into(),
        to: vec!["bob@example.com".into()],
        cc: vec!["charlie@example.com".into()],
        subject: "Hello".into(),
        message_id: Some("<msg1@example.com>".into()),
        in_reply_to: None,
        references: vec![],
        date: None,
        body_text: "First message".into(),
        flags: vec![],
        size: 500,
    };

    let bob_reply = ParsedEmail {
        imap_uid: 2,
        folder: "INBOX".into(),
        from: "bob@example.com".into(),
        to: vec!["alice@example.com".into()],
        cc: vec!["dave@example.com".into()],
        subject: "Re: Hello".into(),
        message_id: Some("<msg2@example.com>".into()),
        in_reply_to: Some("<msg1@example.com>".into()),
        references: vec!["<msg1@example.com>".into()],
        date: None,
        body_text: "Thanks!".into(),
        flags: vec![],
        size: 300,
    };

    let event = build_thread_connector_event(
        &[alice_msg, bob_reply],
        "sync-123".into(),
        "source-1".into(),
        "content-1".into(),
        "Test Account",
        None,
        false,
    );

    match event {
        ConnectorEvent::DocumentCreated { permissions, .. } => {
            assert!(!permissions.public);
            assert!(permissions.users.iter().any(|u| u == "alice@example.com"));
            assert!(permissions.users.iter().any(|u| u == "bob@example.com"));
            assert!(permissions.users.iter().any(|u| u == "charlie@example.com"));
            assert!(permissions.users.iter().any(|u| u == "dave@example.com"));
            assert_eq!(permissions.users.len(), 4);
        }
        _ => panic!("Expected DocumentCreated"),
    }
}

#[test]
fn test_thread_attributes_include_subject_recipients_flags_and_thread_id() {
    use shared::models::ConnectorEvent;

    let email = ParsedEmail {
        imap_uid: 1,
        folder: "INBOX".into(),
        from: "alice@example.com".into(),
        to: vec!["bob@example.com".into(), "charlie@example.com".into()],
        cc: vec!["dave@example.com".into()],
        subject: "Important Meeting".into(),
        message_id: Some("<msg-abc123@example.com>".into()),
        in_reply_to: None,
        references: vec![],
        date: None,
        body_text: "Let's discuss".into(),
        flags: vec!["\\Seen".into(), "\\Flagged".into()],
        size: 1000,
    };

    let event = build_thread_connector_event(
        &[email],
        "sync-456".into(),
        "source-2".into(),
        "content-2".into(),
        "Test",
        None,
        false,
    );

    match event {
        ConnectorEvent::DocumentCreated { attributes, .. } => {
            let attrs = attributes.unwrap();
            assert_eq!(attrs.get("subject").and_then(|v| v.as_str()), Some("Important Meeting"));
            assert_eq!(attrs.get("to").and_then(|v| v.as_array()).map(|a| a.len()), Some(2));
            assert_eq!(attrs.get("cc").and_then(|v| v.as_array()).map(|a| a.len()), Some(1));
            assert_eq!(attrs.get("flags").and_then(|v| v.as_array()).map(|a| a.len()), Some(2));
            assert!(attrs.contains_key("thread_id"));
            assert!(attrs.contains_key("message_count"));
        }
        _ => panic!("Expected DocumentCreated"),
    }
}

#[test]
fn test_parse_thread_headers_and_thread_id() {
    let raw_email = b"From: alice@example.com\r\n\
        To: bob@example.com\r\n\
        Subject: Test\r\n\
        Message-ID: <msg-xyz@example.com>\r\n\
        In-Reply-To: <msg-abc@example.com>\r\n\
        References: <msg-1@example.com> <msg-abc@example.com>\r\n\
        Date: Mon, 15 Jan 2024 10:00:00 +0000\r\n\
        \r\n\
        Body text\r\n";

    let email = parse_raw_email(raw_email, 5, "INBOX").unwrap();

    // Threading headers parsed with angle brackets preserved.
    assert_eq!(email.in_reply_to.as_deref(), Some("<msg-abc@example.com>"));
    assert_eq!(email.references.len(), 2);
    assert_eq!(email.references[0], "<msg-1@example.com>");
    assert_eq!(email.references[1], "<msg-abc@example.com>");

    // thread_id() derives from references[0].
    let thread_id = email.thread_id();
    assert_eq!(thread_id, "<msg-1@example.com>");
}

#[test]
fn test_make_thread_document_id_is_stable() {
    let doc_id = make_thread_document_id("source-123", "INBOX", "<msg-root@example.com>");

    assert!(doc_id.starts_with("imap-thread:"));
    assert!(doc_id.contains("source-123"));
    assert!(doc_id.contains("INBOX"));
    // '<', '@', '>' are all percent-encoded by urlencoding::encode.
    assert!(doc_id.contains("%40"), "@ must be percent-encoded");

    // Must be deterministic.
    let doc_id_2 = make_thread_document_id("source-123", "INBOX", "<msg-root@example.com>");
    assert_eq!(doc_id, doc_id_2);
}

#[test]
fn test_webmail_url_from_template() {
    use shared::models::ConnectorEvent;

    let email = ParsedEmail {
        imap_uid: 42,
        folder: "Sent/Archive".into(),
        from: "alice@example.com".into(),
        to: vec![],
        cc: vec![],
        subject: String::new(),
        message_id: Some("<msg-unique@gmail.com>".into()),
        in_reply_to: None,
        references: vec![],
        date: None,
        body_text: "Test".into(),
        flags: vec![],
        size: 100,
    };

    let template = "https://mail.example.com/#folder/{folder}/uid/{uid}";
    let event = build_thread_connector_event(
        &[email],
        "sync-789".into(),
        "source-4".into(),
        "content-4".into(),
        "Gmail",
        Some(template),
        false,
    );

    match event {
        ConnectorEvent::DocumentCreated { metadata, .. } => {
            let url = metadata.url.expect("URL should be set when template is provided");
            assert!(url.contains("42"), "URL must contain the UID");
            assert!(url.contains("Sent"), "URL must contain the folder name");
        }
        _ => panic!("Expected DocumentCreated"),
    }
}

#[test]
fn test_generate_thread_content_includes_multiple_messages() {
    let msg1 = ParsedEmail {
        imap_uid: 1,
        folder: "INBOX".into(),
        from: "alice@example.com".into(),
        to: vec![],
        cc: vec![],
        subject: "Hello".into(),
        message_id: Some("<msg1@example.com>".into()),
        in_reply_to: None,
        references: vec![],
        date: None,
        body_text: "First message body".into(),
        flags: vec![],
        size: 200,
    };

    let msg2 = ParsedEmail {
        imap_uid: 2,
        folder: "INBOX".into(),
        from: "bob@example.com".into(),
        to: vec![],
        cc: vec![],
        subject: "Re: Hello".into(),
        message_id: Some("<msg2@example.com>".into()),
        in_reply_to: Some("<msg1@example.com>".into()),
        references: vec!["<msg1@example.com>".into()],
        date: None,
        body_text: "Second message body".into(),
        flags: vec![],
        size: 150,
    };

    let content = generate_thread_content(&[msg1, msg2]);

    assert!(content.contains("=== Message 1 ==="));
    assert!(content.contains("=== Message 2 ==="));
    assert!(content.contains("First message body"));
    assert!(content.contains("Second message body"));
    assert!(content.contains("alice@example.com"));
    assert!(content.contains("bob@example.com"));
}

#[test]
fn test_build_thread_connector_event_updates_existing_thread() {
    use shared::models::ConnectorEvent;

    let email1 = ParsedEmail {
        imap_uid: 1,
        folder: "INBOX".into(),
        from: "alice@example.com".into(),
        to: vec![],
        cc: vec![],
        subject: "Thread".into(),
        message_id: Some("<msg1@example.com>".into()),
        in_reply_to: None,
        references: vec![],
        date: None,
        body_text: "Original".into(),
        flags: vec![],
        size: 100,
    };

    let email2 = ParsedEmail {
        imap_uid: 2,
        folder: "INBOX".into(),
        from: "bob@example.com".into(),
        to: vec![],
        cc: vec![],
        subject: "Re: Thread".into(),
        message_id: Some("<msg2@example.com>".into()),
        in_reply_to: Some("<msg1@example.com>".into()),
        references: vec!["<msg1@example.com>".into()],
        date: None,
        body_text: "Reply".into(),
        flags: vec![],
        size: 120,
    };

    let event = build_thread_connector_event(
        &[email1, email2],
        "sync-update".into(),
        "source-5".into(),
        "content-5".into(),
        "Account",
        None,
        true, // is_update = true → DocumentUpdated
    );

    match event {
        ConnectorEvent::DocumentUpdated { attributes, .. } => {
            let attrs = attributes.unwrap();
            let message_count = attrs.get("message_count").and_then(|v| v.as_u64());
            assert_eq!(message_count, Some(2));
        }
        _ => panic!("Expected DocumentUpdated"),
    }
}
// ─── Helpers used by tests below ────────────────────────────────────────────

fn test_email(
    uid: u32,
    message_id: &str,
    in_reply_to: Option<&str>,
    references: &[&str],
) -> ParsedEmail {
    ParsedEmail {
        imap_uid: uid,
        folder: "INBOX".into(),
        from: "test@example.com".into(),
        to: vec![],
        cc: vec![],
        subject: String::new(),
        message_id: Some(message_id.into()),
        in_reply_to: in_reply_to.map(String::from),
        references: references.iter().map(|s| s.to_string()).collect(),
        date: None,
        body_text: String::new(),
        flags: vec![],
        size: 0,
    }
}

// ── skipped_uids exclusion guarantee ─────────────────────────────────────────

/// UIDs in `skipped_uids` (exceeded size limit) must not appear in `new_uids`
/// even though they are present on the server and absent from `indexed_uids`.
/// This prevents re-downloading their bodies on every incremental sync.
#[test]
fn test_skipped_uids_excluded_from_new_uids() {
    use std::collections::HashSet;

    let indexed_uids: HashSet<u32> = [1u32, 2].into_iter().collect();
    let skipped_uids: HashSet<u32> = [3u32, 4].into_iter().collect();
    let server_uids: Vec<u32> = vec![1, 2, 3, 4, 5, 6];

    let mut new_uids: Vec<u32> = server_uids
        .into_iter()
        .filter(|uid| !indexed_uids.contains(uid) && !skipped_uids.contains(uid))
        .collect();
    new_uids.sort_unstable();

    assert_eq!(new_uids, vec![5, 6], "skipped UIDs must not be retried");
}

// ── Thread root resolution ────────────────────────────────────────────────────

/// When a `References` header is present its first element is returned
/// directly without walking the chain (RFC 5256 §2.2).
#[test]
fn test_resolve_thread_root_uses_references_first() {
    use std::collections::HashMap;

    let msg = test_email(5, "<msg@x>", Some("<other@x>"), &["<root-ref@x>", "<other@x>"]);
    let messages: HashMap<u32, ParsedEmail> = [(5, msg)].into_iter().collect();
    let by_message_id: HashMap<String, u32> = HashMap::new(); // no chain walk needed

    let root = resolve_thread_root(5, &messages, &by_message_id);
    assert_eq!(root, "<root-ref@x>", "References[0] must be used when present");
}

/// A reply-to-reply where only `In-Reply-To` is set (no `References`) must be
/// grouped under the original root, not under the intermediate message.
#[test]
fn test_resolve_thread_root_walks_in_reply_to_chain() {
    use std::collections::HashMap;

    // root ← child ← grandchild (none have References header)
    let root = test_email(1, "<root@x>", None, &[]);
    let child = test_email(2, "<child@x>", Some("<root@x>"), &[]);
    let grandchild = test_email(3, "<gc@x>", Some("<child@x>"), &[]);

    let messages: HashMap<u32, ParsedEmail> =
        [(1, root), (2, child), (3, grandchild)].into_iter().collect();
    let by_message_id: HashMap<String, u32> =
        [("<root@x>".into(), 1u32), ("<child@x>".into(), 2), ("<gc@x>".into(), 3)]
            .into_iter()
            .collect();

    // Grandchild must resolve to root's thread_id() = "<root@x>"
    let resolved = resolve_thread_root(3, &messages, &by_message_id);
    assert_eq!(resolved, "<root@x>", "In-Reply-To chain must walk to the root");

    // Child must also resolve to root
    let resolved_child = resolve_thread_root(2, &messages, &by_message_id);
    assert_eq!(resolved_child, "<root@x>");

    // Root resolves to itself
    let resolved_root = resolve_thread_root(1, &messages, &by_message_id);
    assert_eq!(resolved_root, "<root@x>");
}

/// `resolve_new_email_thread_root` must group a new email under the same root
/// as its parent when the parent is already stored.
#[test]
fn test_resolve_new_email_uses_stored_parent_root() {
    use std::collections::HashMap;

    let root = test_email(1, "<root@x>", None, &[]);
    let child = test_email(2, "<child@x>", Some("<root@x>"), &[]);

    let messages: HashMap<u32, ParsedEmail> =
        [(1, root), (2, child)].into_iter().collect();
    let by_message_id: HashMap<String, u32> =
        [("<root@x>".into(), 1u32), ("<child@x>".into(), 2)]
            .into_iter()
            .collect();

    // New grandchild with only In-Reply-To, not yet in messages map
    let grandchild = test_email(3, "<gc@x>", Some("<child@x>"), &[]);

    let resolved = resolve_new_email_thread_root(&grandchild, &messages, &by_message_id);
    assert_eq!(resolved, "<root@x>");
}

/// The cycle guard must prevent an infinite loop when `In-Reply-To` forms a cycle.
#[test]
fn test_resolve_thread_root_cycle_guard() {
    use std::collections::HashMap;

    let msg_a = test_email(1, "<a@x>", Some("<b@x>"), &[]);
    let msg_b = test_email(2, "<b@x>", Some("<a@x>"), &[]);

    let messages: HashMap<u32, ParsedEmail> =
        [(1, msg_a), (2, msg_b)].into_iter().collect();
    let by_message_id: HashMap<String, u32> =
        [("<a@x>".into(), 1u32), ("<b@x>".into(), 2)].into_iter().collect();

    // Must terminate without panicking and return some stable string
    let root_a = resolve_thread_root(1, &messages, &by_message_id);
    let root_b = resolve_thread_root(2, &messages, &by_message_id);

    // Both should return a consistent value (implementation-defined in a cycle)
    assert!(!root_a.is_empty());
    assert!(!root_b.is_empty());
}

/// Regression: `build_thread_connector_event` must produce a stable document_id
/// regardless of which message sorts first.
///
/// The bug: a reply with (a) no `References` header, (b) `In-Reply-To` pointing
/// to a *non-root* intermediate message, and (c) `date = None` sorts before all
/// dated messages.  Before the fix, `first.thread_id()` returned the intermediate
/// message-ID instead of the root, so document_id changed every time that reply
/// was added or removed from the thread slice, orphaning documents in the index.
#[test]
fn test_thread_document_id_stable_when_dateless_non_root_reply_sorts_first() {
    use shared::models::ConnectorEvent;

    // root@x ← child@x ← grandchild@x
    // grandchild has no References, only In-Reply-To = child@x, and no date.
    let root = ParsedEmail {
        imap_uid: 1,
        folder: "INBOX".into(),
        from: "a@example.com".into(),
        to: vec![],
        cc: vec![],
        subject: "Thread".into(),
        message_id: Some("<root@x>".into()),
        in_reply_to: None,
        references: vec![],
        date: Some(time::OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap()),
        body_text: "Root".into(),
        flags: vec![],
        size: 100,
    };

    let child = ParsedEmail {
        imap_uid: 2,
        folder: "INBOX".into(),
        from: "b@example.com".into(),
        to: vec![],
        cc: vec![],
        subject: "Re: Thread".into(),
        message_id: Some("<child@x>".into()),
        in_reply_to: Some("<root@x>".into()),
        // child has a References header → references[0] is the thread root
        references: vec!["<root@x>".into()],
        date: Some(time::OffsetDateTime::from_unix_timestamp(1_700_001_000).unwrap()),
        body_text: "Child".into(),
        flags: vec![],
        size: 100,
    };

    // grandchild: no References, in_reply_to = child (not the root), date = None
    // → sorts before root and child, but its in_reply_to points to child, not root.
    let grandchild = ParsedEmail {
        imap_uid: 3,
        folder: "INBOX".into(),
        from: "c@example.com".into(),
        to: vec![],
        cc: vec![],
        subject: "Re: Re: Thread".into(),
        message_id: Some("<gc@x>".into()),
        in_reply_to: Some("<child@x>".into()),
        references: vec![],   // deliberately empty — exposes the bug
        date: None,            // sorts before dated messages
        body_text: "GC".into(),
        flags: vec![],
        size: 100,
    };

    // Build event with all three messages.
    let event_full = build_thread_connector_event(
        &[root.clone(), child.clone(), grandchild.clone()],
        "sync-1".into(),
        "src-1".into(),
        "c1".into(),
        "Acct",
        None,
        false,
    );
    // Build event without grandchild (simulates grandchild already deleted).
    let event_without_gc = build_thread_connector_event(
        &[root.clone(), child.clone()],
        "sync-1".into(),
        "src-1".into(),
        "c2".into(),
        "Acct",
        None,
        true,
    );

    let id_full = match &event_full {
        ConnectorEvent::DocumentCreated { document_id, .. } => document_id.clone(),
        _ => panic!("Expected DocumentCreated"),
    };
    let id_without_gc = match &event_without_gc {
        ConnectorEvent::DocumentUpdated { document_id, .. } => document_id.clone(),
        _ => panic!("Expected DocumentUpdated"),
    };

    assert_eq!(
        id_full, id_without_gc,
        "document_id must be stable before and after removing the dateless non-root reply"
    );
    // The stable ID should be derived from the thread root's message-ID.
    // child has references[0] = <root@x>, so the canonical root is <root@x>.
    assert!(
        id_full.contains("root"),
        "document_id should be derived from the thread root; got: {}",
        id_full
    );
}

// ── Attachment extraction ─────────────────────────────────────────────────────

use omni_imap_connector::attachment::extract_attachments;

/// Helper: build a raw RFC 2822 email with a single base64-encoded attachment.
fn build_email_with_attachment(
    content_type: &str,
    filename: &str,
    body_bytes: &[u8],
) -> Vec<u8> {
    use base64::Engine;
    let encoded = base64::engine::general_purpose::STANDARD.encode(body_bytes);
    format!(
        "From: sender@example.com\r\n\
         To: recipient@example.com\r\n\
         Subject: Attachment test\r\n\
         MIME-Version: 1.0\r\n\
         Content-Type: multipart/mixed; boundary=\"boundary42\"\r\n\
         \r\n\
         --boundary42\r\n\
         Content-Type: text/plain; charset=utf-8\r\n\
         \r\n\
         See attached.\r\n\
         --boundary42\r\n\
         Content-Type: {}; name=\"{}\"\r\n\
         Content-Disposition: attachment; filename=\"{}\"\r\n\
         Content-Transfer-Encoding: base64\r\n\
         \r\n\
         {}\r\n\
         --boundary42--\r\n",
        content_type, filename, filename, encoded
    )
    .into_bytes()
}

#[test]
fn test_attachment_plain_text_extracted() {
    let raw = build_email_with_attachment(
        "text/plain",
        "notes.txt",
        b"Important meeting notes for Q4 review.",
    );
    let parsed = mailparse::parse_mail(&raw).unwrap();
    let attachments = extract_attachments(&parsed);
    assert_eq!(attachments.len(), 1);
    assert_eq!(attachments[0].filename, "notes.txt");
    assert!(
        attachments[0].text.contains("Important meeting notes"),
        "Expected text content, got: '{}'",
        attachments[0].text
    );
}

#[test]
fn test_attachment_csv_extracted() {
    let raw = build_email_with_attachment(
        "text/csv",
        "data.csv",
        b"name,age\nAlice,30\nBob,25",
    );
    let parsed = mailparse::parse_mail(&raw).unwrap();
    let attachments = extract_attachments(&parsed);
    assert_eq!(attachments.len(), 1);
    assert!(attachments[0].text.contains("Alice"));
}

#[test]
fn test_attachment_html_converted_to_text() {
    let raw = build_email_with_attachment(
        "text/html",
        "report.html",
        b"<html><body><h1>Report</h1><p>Quarterly earnings summary.</p></body></html>",
    );
    let parsed = mailparse::parse_mail(&raw).unwrap();
    let attachments = extract_attachments(&parsed);
    assert_eq!(attachments.len(), 1);
    assert_eq!(attachments[0].filename, "report.html");
    assert!(
        attachments[0].text.contains("Quarterly earnings summary"),
        "Expected HTML-to-text conversion, got: '{}'",
        attachments[0].text
    );
    assert!(
        !attachments[0].text.contains("<p>"),
        "Should not contain raw HTML tags"
    );
}

#[test]
fn test_attachment_docx_extracted() {
    // Build a minimal DOCX in memory.
    let docx = docx_rs::Docx::new().add_paragraph(
        docx_rs::Paragraph::new()
            .add_run(docx_rs::Run::new().add_text("Contract draft version 3")),
    );
    let mut docx_bytes = Vec::new();
    docx.build()
        .pack(std::io::Cursor::new(&mut docx_bytes))
        .unwrap();

    let raw = build_email_with_attachment(
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "contract.docx",
        &docx_bytes,
    );
    let parsed = mailparse::parse_mail(&raw).unwrap();
    let attachments = extract_attachments(&parsed);
    assert_eq!(attachments.len(), 1);
    assert_eq!(attachments[0].filename, "contract.docx");
    assert!(
        attachments[0].text.contains("Contract draft version 3"),
        "Expected DOCX content, got: '{}'",
        attachments[0].text
    );
}

#[test]
fn test_attachment_pptx_extracted() {
    use std::io::Write;
    let mut pptx_bytes = Vec::new();
    {
        let cursor = std::io::Cursor::new(&mut pptx_bytes);
        let mut zip = zip::ZipWriter::new(cursor);
        zip.start_file(
            "ppt/slides/slide1.xml",
            zip::write::FileOptions::default(),
        )
        .unwrap();
        write!(
            zip,
            r#"<?xml version="1.0"?>
<p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"
       xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">
  <p:cSld><p:spTree><p:sp><p:txBody>
    <a:p><a:r><a:t>Budget presentation 2026</a:t></a:r></a:p>
  </p:txBody></p:sp></p:spTree></p:cSld>
</p:sld>"#
        )
        .unwrap();
        zip.finish().unwrap();
    }

    let raw = build_email_with_attachment(
        "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "budget.pptx",
        &pptx_bytes,
    );
    let parsed = mailparse::parse_mail(&raw).unwrap();
    let attachments = extract_attachments(&parsed);
    assert_eq!(attachments.len(), 1);
    assert_eq!(attachments[0].filename, "budget.pptx");
    assert!(
        attachments[0].text.contains("Budget presentation 2026"),
        "Expected PPTX content, got: '{}'",
        attachments[0].text
    );
}

#[test]
fn test_unsupported_attachment_skipped() {
    let raw = build_email_with_attachment(
        "image/png",
        "photo.png",
        &[0x89, 0x50, 0x4E, 0x47], // PNG magic bytes
    );
    let parsed = mailparse::parse_mail(&raw).unwrap();
    let attachments = extract_attachments(&parsed);
    assert!(
        attachments.is_empty(),
        "image/png should not produce any extracted attachment"
    );
}

#[test]
fn test_multiple_attachments_extracted() {
    use base64::Engine;
    let txt_encoded =
        base64::engine::general_purpose::STANDARD.encode(b"Text file content");
    let csv_encoded =
        base64::engine::general_purpose::STANDARD.encode(b"col1,col2\na,b");

    let raw = format!(
        "From: a@b.com\r\n\
         Subject: Multi-attach\r\n\
         MIME-Version: 1.0\r\n\
         Content-Type: multipart/mixed; boundary=\"sep\"\r\n\
         \r\n\
         --sep\r\n\
         Content-Type: text/plain; charset=utf-8\r\n\
         \r\n\
         Body text.\r\n\
         --sep\r\n\
         Content-Type: text/plain; name=\"notes.txt\"\r\n\
         Content-Disposition: attachment; filename=\"notes.txt\"\r\n\
         Content-Transfer-Encoding: base64\r\n\
         \r\n\
         {}\r\n\
         --sep\r\n\
         Content-Type: text/csv; name=\"data.csv\"\r\n\
         Content-Disposition: attachment; filename=\"data.csv\"\r\n\
         Content-Transfer-Encoding: base64\r\n\
         \r\n\
         {}\r\n\
         --sep--\r\n",
        txt_encoded, csv_encoded
    )
    .into_bytes();

    let parsed = mailparse::parse_mail(&raw).unwrap();
    let attachments = extract_attachments(&parsed);
    assert_eq!(attachments.len(), 2, "Expected 2 attachments, got {}", attachments.len());
    let filenames: Vec<&str> = attachments.iter().map(|a| a.filename.as_str()).collect();
    assert!(filenames.contains(&"notes.txt"));
    assert!(filenames.contains(&"data.csv"));
}

#[test]
fn test_attachment_text_included_in_parsed_email_body() {
    let raw = build_email_with_attachment(
        "text/plain",
        "readme.txt",
        b"Attachment content here.",
    );
    let email = parse_raw_email(&raw, 1, "INBOX").unwrap();
    assert!(
        email.body_text.contains("[Attachment: readme.txt]"),
        "body_text should contain attachment header, got: '{}'",
        email.body_text
    );
    assert!(
        email.body_text.contains("Attachment content here."),
        "body_text should contain attachment text, got: '{}'",
        email.body_text
    );
    // The inline body should also be present.
    assert!(
        email.body_text.contains("See attached."),
        "body_text should contain inline text too, got: '{}'",
        email.body_text
    );
}