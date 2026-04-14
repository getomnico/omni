//! Integration tests for the IMAP connector.
//!
//! These tests validate the end-to-end flow from raw email bytes through
//! parsing, attachment extraction, thread assembly, and connector event
//! generation — exercising multiple modules working together as they would
//! in a real sync.

use omni_imap_connector::models::{
    build_thread_connector_event, collect_raw_attachments, generate_thread_content,
    make_thread_document_id, parse_raw_email, resolve_new_email_thread_root, resolve_thread_root,
    FolderSyncState, ImapConnectorState,
};
use shared::models::ConnectorEvent;
use std::collections::HashMap;

/// Helper: build a raw RFC 2822 email with optional attachment.
fn build_raw_email(
    message_id: &str,
    in_reply_to: Option<&str>,
    references: &[&str],
    from: &str,
    to: &str,
    subject: &str,
    body: &str,
    date: Option<&str>,
) -> Vec<u8> {
    let mut headers = format!(
        "From: {}\r\n\
         To: {}\r\n\
         Subject: {}\r\n\
         Message-ID: {}\r\n",
        from, to, subject, message_id
    );

    if let Some(irt) = in_reply_to {
        headers.push_str(&format!("In-Reply-To: {}\r\n", irt));
    }
    if !references.is_empty() {
        headers.push_str(&format!("References: {}\r\n", references.join(" ")));
    }
    if let Some(d) = date {
        headers.push_str(&format!("Date: {}\r\n", d));
    }
    headers.push_str("Content-Type: text/plain; charset=utf-8\r\n\r\n");
    headers.push_str(body);
    headers.push_str("\r\n");

    headers.into_bytes()
}

/// Integration test: Simulate an incremental sync scenario where emails arrive
/// in batches, building up thread state just as the real sync_folder would.
///
/// This exercises:
/// - parse_raw_email (models + attachment)
/// - resolve_new_email_thread_root / resolve_thread_root (threading logic)
/// - generate_thread_content (content assembly)
/// - build_thread_connector_event (event generation)
/// - ImapConnectorState / FolderSyncState (state management)
#[test]
fn test_incremental_sync_flow_builds_thread_correctly() {
    // === Batch 1: Root email arrives ===
    let raw_root = build_raw_email(
        "<root@example.com>",
        None,
        &[],
        "alice@example.com",
        "bob@example.com",
        "Project Discussion",
        "Let's discuss the project timeline.",
        Some("Mon, 15 Jan 2024 10:00:00 +0000"),
    );

    let email_root = parse_raw_email(&raw_root, 1, "INBOX").unwrap();
    assert_eq!(email_root.subject, "Project Discussion");
    assert_eq!(email_root.thread_id(), "<root@example.com>");

    // Simulate connector state after indexing root
    let mut folder_state = FolderSyncState {
        uid_validity: 12345,
        indexed_uids: vec![1],
        messages: HashMap::from([(1, email_root.clone())]),
        skipped_uids: Default::default(),
    };

    let mut by_message_id: HashMap<String, u32> = HashMap::from([
        ("<root@example.com>".to_string(), 1),
    ]);

    // Build initial thread event (single message)
    let event1 = build_thread_connector_event(
        &[email_root.clone()],
        "sync-1".into(),
        "source-1".into(),
        "content-1".into(),
        "Test Account",
        None,
        Some("alice@example.com"),
        false,
    );

    let doc_id_1 = match &event1 {
        ConnectorEvent::DocumentCreated { document_id, .. } => document_id.clone(),
        _ => panic!("Expected DocumentCreated"),
    };

    // === Batch 2: Reply arrives ===
    let raw_reply = build_raw_email(
        "<reply1@example.com>",
        Some("<root@example.com>"),
        &["<root@example.com>"],
        "bob@example.com",
        "alice@example.com",
        "Re: Project Discussion",
        "Sounds good, let's meet Tuesday.",
        Some("Mon, 15 Jan 2024 11:00:00 +0000"),
    );

    let email_reply = parse_raw_email(&raw_reply, 2, "INBOX").unwrap();

    // Resolve thread root for the new email (simulates sync_folder logic)
    let thread_root = resolve_new_email_thread_root(
        &email_reply,
        &folder_state.messages,
        &by_message_id,
    );
    assert_eq!(thread_root, "<root@example.com>", "Reply should resolve to root");

    // Update state (simulates successful indexing)
    folder_state.indexed_uids.push(2);
    folder_state.messages.insert(2, email_reply.clone());
    by_message_id.insert("<reply1@example.com>".to_string(), 2);

    // Build updated thread event
    let thread_messages = vec![email_root.clone(), email_reply.clone()];
    let event2 = build_thread_connector_event(
        &thread_messages,
        "sync-2".into(),
        "source-1".into(),
        "content-2".into(),
        "Test Account",
        None,
        Some("alice@example.com"),
        true, // is_update because thread already existed
    );

    let doc_id_2 = match &event2 {
        ConnectorEvent::DocumentUpdated { document_id, attributes, .. } => {
            // Verify message count updated
            let msg_count = attributes.as_ref()
                .and_then(|a| a.get("message_count"))
                .and_then(|v| v.as_u64());
            assert_eq!(msg_count, Some(2), "Thread should have 2 messages");
            document_id.clone()
        }
        _ => panic!("Expected DocumentUpdated"),
    };

    // Critical: document_id must be stable across updates
    assert_eq!(doc_id_1, doc_id_2, "Thread document_id must be stable");

    // === Batch 3: Nested reply (reply-to-reply without References header) ===
    // This is the tricky case: grandchild only has In-Reply-To pointing to reply, not root
    let raw_grandchild = build_raw_email(
        "<grandchild@example.com>",
        Some("<reply1@example.com>"),
        &[], // No References header - must walk the chain
        "alice@example.com",
        "bob@example.com",
        "Re: Re: Project Discussion",
        "Tuesday works for me.",
        Some("Mon, 15 Jan 2024 12:00:00 +0000"),
    );

    let email_grandchild = parse_raw_email(&raw_grandchild, 3, "INBOX").unwrap();

    // Thread resolution must walk the In-Reply-To chain
    let thread_root_gc = resolve_new_email_thread_root(
        &email_grandchild,
        &folder_state.messages,
        &by_message_id,
    );
    assert_eq!(
        thread_root_gc, "<root@example.com>",
        "Grandchild must resolve to original root via chain-walking"
    );

    // Update state
    folder_state.indexed_uids.push(3);
    folder_state.messages.insert(3, email_grandchild.clone());
    by_message_id.insert("<grandchild@example.com>".to_string(), 3);

    // Final thread with all 3 messages
    let all_messages = vec![email_root, email_reply, email_grandchild];
    let content = generate_thread_content(&all_messages);

    // Content should include all messages in chronological order
    assert!(content.contains("=== Message 1 ==="));
    assert!(content.contains("=== Message 2 ==="));
    assert!(content.contains("=== Message 3 ==="));
    assert!(content.contains("Let's discuss the project timeline."));
    assert!(content.contains("Sounds good, let's meet Tuesday."));
    assert!(content.contains("Tuesday works for me."));

    let event3 = build_thread_connector_event(
        &all_messages,
        "sync-3".into(),
        "source-1".into(),
        "content-3".into(),
        "Test Account",
        None,
        Some("alice@example.com"),
        true,
    );

    let doc_id_3 = match &event3 {
        ConnectorEvent::DocumentUpdated { document_id, .. } => document_id.clone(),
        _ => panic!("Expected DocumentUpdated"),
    };

    assert_eq!(doc_id_1, doc_id_3, "Document ID must remain stable after 3 messages");
}

/// Integration test: State persistence and restoration simulates connector restart.
#[test]
fn test_connector_state_persistence_across_restarts() {
    // Build initial state with some indexed messages
    let raw1 = build_raw_email(
        "<msg1@example.com>",
        None,
        &[],
        "a@example.com",
        "b@example.com",
        "Test",
        "Body 1",
        Some("Mon, 01 Jan 2024 10:00:00 +0000"),
    );
    let email1 = parse_raw_email(&raw1, 100, "INBOX").unwrap();

    let raw2 = build_raw_email(
        "<msg2@example.com>",
        Some("<msg1@example.com>"),
        &["<msg1@example.com>"],
        "b@example.com",
        "a@example.com",
        "Re: Test",
        "Body 2",
        Some("Mon, 01 Jan 2024 11:00:00 +0000"),
    );
    let email2 = parse_raw_email(&raw2, 101, "INBOX").unwrap();

    let mut state = ImapConnectorState::default();
    state.folders.insert(
        "INBOX".to_string(),
        FolderSyncState {
            uid_validity: 999,
            indexed_uids: vec![100, 101],
            messages: HashMap::from([(100, email1.clone()), (101, email2.clone())]),
            skipped_uids: [102].into_iter().collect(), // One message skipped due to size
        },
    );

    // Serialize (simulates persisting to database)
    let json = state.to_json();

    // Deserialize (simulates loading after restart)
    let restored = ImapConnectorState::from_connector_state(&Some(json));

    // Verify state integrity
    let inbox = restored.folders.get("INBOX").expect("INBOX should exist");
    assert_eq!(inbox.uid_validity, 999);
    assert_eq!(inbox.indexed_uids, vec![100, 101]);
    assert!(inbox.skipped_uids.contains(&102));
    assert_eq!(inbox.messages.len(), 2);

    // Thread resolution should work with restored state
    let by_message_id: HashMap<String, u32> = inbox
        .messages
        .values()
        .filter_map(|m| m.message_id.as_ref().map(|id| (id.clone(), m.imap_uid)))
        .collect();

    let thread_root = resolve_thread_root(101, &inbox.messages, &by_message_id);
    assert_eq!(thread_root, "<msg1@example.com>");
}

/// Integration test: Deletion detection logic with partial failures.
///
/// This tests the critical invariant that failed deletion events keep UIDs
/// in indexed_uids for retry, while successful deletions remove them.
#[test]
fn test_deletion_detection_with_partial_failures() {
    // Initial state: UIDs 1-5 indexed
    let mut indexed_uids: Vec<u32> = vec![1, 2, 3, 4, 5];

    // Server state: only UIDs 1, 3, 5 remain (2 and 4 deleted)
    let server_uids: std::collections::HashSet<u32> = [1, 3, 5].into_iter().collect();

    // Detect deletions
    let deleted_uids: Vec<u32> = indexed_uids
        .iter()
        .copied()
        .filter(|uid| !server_uids.contains(uid))
        .collect();
    assert_eq!(deleted_uids, vec![2, 4]);

    // Simulate: deletion event for UID 2 succeeds, UID 4 fails
    let mut failed_deletion_uids: std::collections::HashSet<u32> = std::collections::HashSet::new();
    for uid in &deleted_uids {
        if *uid == 4 {
            // Simulated network error
            failed_deletion_uids.insert(*uid);
        }
        // UID 2 succeeds (not added to failed set)
    }

    // Apply the retain logic (mirrors sync_folder implementation)
    indexed_uids.retain(|uid| server_uids.contains(uid) || failed_deletion_uids.contains(uid));

    // Verify: UID 2 removed (success), UID 4 retained (for retry)
    assert!(!indexed_uids.contains(&2), "UID 2 should be removed after successful deletion");
    assert!(indexed_uids.contains(&4), "UID 4 should be retained for retry after failure");
    assert_eq!(indexed_uids, vec![1, 3, 4, 5]);
}

/// Integration test: UIDVALIDITY change triggers full resync.
#[test]
fn test_uidvalidity_change_clears_state() {
    let mut folder_state = FolderSyncState {
        uid_validity: 12345,
        indexed_uids: vec![1, 2, 3],
        messages: {
            let mut m = HashMap::new();
            let raw = build_raw_email(
                "<test@example.com>",
                None,
                &[],
                "a@b.com",
                "c@d.com",
                "Test",
                "Body",
                None,
            );
            m.insert(1, parse_raw_email(&raw, 1, "INBOX").unwrap());
            m
        },
        skipped_uids: [99].into_iter().collect(),
    };

    // Server now reports different UIDVALIDITY
    let server_uid_validity = 99999u32;

    // Apply UIDVALIDITY change logic (mirrors sync_folder)
    if server_uid_validity != 0
        && folder_state.uid_validity != 0
        && folder_state.uid_validity != server_uid_validity
    {
        folder_state.indexed_uids.clear();
        folder_state.messages.clear();
        folder_state.skipped_uids.clear();
    }
    if server_uid_validity != 0 {
        folder_state.uid_validity = server_uid_validity;
    }

    assert_eq!(folder_state.uid_validity, 99999);
    assert!(folder_state.indexed_uids.is_empty(), "indexed_uids must be cleared");
    assert!(folder_state.messages.is_empty(), "messages must be cleared");
    assert!(folder_state.skipped_uids.is_empty(), "skipped_uids must be cleared");
}

/// Integration test: Email with attachment flows through the full pipeline.
#[test]
fn test_email_with_attachment_end_to_end() {
    use base64::Engine;

    let attachment_content = b"Q4 Revenue Report\nTotal: $1.2M";
    let encoded = base64::engine::general_purpose::STANDARD.encode(attachment_content);

    let raw = format!(
        "From: finance@example.com\r\n\
         To: ceo@example.com\r\n\
         Subject: Q4 Financial Report\r\n\
         Message-ID: <finance-q4@example.com>\r\n\
         Date: Wed, 15 Jan 2025 09:00:00 +0000\r\n\
         MIME-Version: 1.0\r\n\
         Content-Type: multipart/mixed; boundary=\"boundary123\"\r\n\
         \r\n\
         --boundary123\r\n\
         Content-Type: text/plain; charset=utf-8\r\n\
         \r\n\
         Please find attached the Q4 report.\r\n\
         --boundary123\r\n\
         Content-Type: text/plain; name=\"report.txt\"\r\n\
         Content-Disposition: attachment; filename=\"report.txt\"\r\n\
         Content-Transfer-Encoding: base64\r\n\
         \r\n\
         {}\r\n\
         --boundary123--\r\n",
        encoded
    )
    .into_bytes();

    // Parse email — body_text contains only inline body, not attachments
    let mut email = parse_raw_email(&raw, 1, "Finance").unwrap();

    // Plain text body → no HTML conversion needed
    assert!(!email.body_is_html);

    // Verify inline body is present but attachment text is NOT (extracted separately)
    assert!(email.body_text.contains("Please find attached"));
    assert!(!email.body_text.contains("Q4 Revenue Report"),
        "attachment text should not be in body_text after parse_raw_email");

    // Collect raw attachments (mirrors what sync.rs does before calling sdk_client.extract_text)
    let parsed_mail = mailparse::parse_mail(&raw).unwrap();
    let raw_attachments = collect_raw_attachments(&parsed_mail);
    assert_eq!(raw_attachments.len(), 1);
    assert_eq!(raw_attachments[0].filename, "report.txt");
    // The raw data should contain the original content
    let attachment_text = String::from_utf8_lossy(&raw_attachments[0].data);
    assert!(attachment_text.contains("Q4 Revenue Report"));

    // Simulate what the sync loop does: extract text and append to body_text.
    // In production the SDK calls the connector-manager (with Docling support).
    // Here we call the built-in extractor directly to complete the pipeline test.
    for att in &raw_attachments {
        let text = shared::content_extractor::extract_content(
            &att.data, &att.mime_type, Some(att.filename.as_str()),
        ).unwrap_or_default();
        if !text.is_empty() {
            email.body_text.push_str(&format!("\n\n[Attachment: {}]\n{}", att.filename, text));
        }
    }

    // Now body_text should contain both inline body and extracted attachment text
    assert!(email.body_text.contains("Q4 Revenue Report"));
    assert!(email.body_text.contains("$1.2M"));
    assert!(email.body_text.contains("[Attachment: report.txt]"));

    // Generate connector event
    let event = build_thread_connector_event(
        &[email],
        "sync-finance".into(),
        "source-fin".into(),
        "content-fin".into(),
        "Finance Mailbox",
        Some("https://mail.example.com/folder/{folder}/uid/{uid}"),
        Some("finance@example.com"),
        false,
    );

    match event {
        ConnectorEvent::DocumentCreated {
            document_id,
            metadata,
            permissions,
            ..
        } => {
            assert!(document_id.starts_with("imap-thread:"));
            assert_eq!(metadata.title.as_deref(), Some("Q4 Financial Report"));
            assert_eq!(metadata.author.as_deref(), Some("finance@example.com"));
            // URL should be populated from template
            assert!(metadata.url.as_ref().unwrap().contains("Finance"));
            assert!(metadata.url.as_ref().unwrap().contains("1"));
            // Permissions should only include owner
            assert_eq!(permissions.users, vec!["finance@example.com"]);
            assert!(!permissions.public);
        }
        _ => panic!("Expected DocumentCreated"),
    }
}

/// Integration test: Thread document ID stability with edge cases.
#[test]
fn test_thread_document_id_stability_edge_cases() {
    // Case 1: Folder with special characters
    let id1 = make_thread_document_id("Work/Projects/2024", "<thread@example.com>");
    let id2 = make_thread_document_id("Work/Projects/2024", "<thread@example.com>");
    assert_eq!(id1, id2, "ID must be deterministic");
    assert!(id1.contains("%2F"), "Slashes must be encoded");

    // Case 2: Thread ID with special characters
    let id3 = make_thread_document_id("INBOX", "<msg+tag@sub.example.com>");
    assert!(id3.contains("%40"), "@ must be encoded");
    assert!(id3.contains("%2B"), "+ must be encoded");

    // Case 3: Unicode folder name
    let id4 = make_thread_document_id("Gelöschte Objekte", "<msg@de.example>");
    let id5 = make_thread_document_id("Gelöschte Objekte", "<msg@de.example>");
    assert_eq!(id4, id5, "Unicode folders must produce stable IDs");

    // Case 4: Same thread across different sources produces the same ID
    let id6 = make_thread_document_id("INBOX", "<list-msg@mailing-list.org>");
    let id7 = make_thread_document_id("INBOX", "<list-msg@mailing-list.org>");
    assert_eq!(id6, id7, "Same folder+thread across sources must produce same ID");
    assert!(!id6.contains("source"), "source_id must not appear in thread document ID");
}
