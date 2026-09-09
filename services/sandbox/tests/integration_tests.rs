mod common;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde_json::{Value, json};

use common::SandboxTestFixture;

// ---------------------------------------------------------------------------
// Execution tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_bash_execution() {
    let f = SandboxTestFixture::shared().await;

    let resp = f
        .client
        .post(f.url("/execute/bash"))
        .json(&json!({ "command": "echo hello", "chat_id": "bash-test" }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["exit_code"], 0);
    assert_eq!(body["stdout"].as_str().unwrap().trim(), "hello");

    // pwd should be the chat dir under /scratch/
    let resp = f
        .client
        .post(f.url("/execute/bash"))
        .json(&json!({ "command": "pwd", "chat_id": "bash-test" }))
        .send()
        .await
        .unwrap();
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["exit_code"], 0);
    let cwd = body["stdout"].as_str().unwrap().trim();
    assert_eq!(cwd, "/scratch/bash-test");
}

#[tokio::test]
async fn test_python_execution_and_cleanup() {
    let f = SandboxTestFixture::shared().await;
    let chat_id = "python-test";

    let resp = f
        .client
        .post(f.url("/execute/python"))
        .json(&json!({ "code": "print('hello')", "chat_id": chat_id }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["exit_code"], 0);
    assert_eq!(body["stdout"].as_str().unwrap().trim(), "hello");

    // _script.py should have been cleaned up
    let resp = f
        .client
        .post(f.url("/files/stat"))
        .json(&json!({ "path": "_script.py", "chat_id": chat_id }))
        .send()
        .await
        .unwrap();
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["exists"], false);
}

#[tokio::test]
async fn test_execution_timeout() {
    let f = SandboxTestFixture::with_timeout(2).await;

    let resp = f
        .client
        .post(f.url("/execute/bash"))
        .json(&json!({ "command": "sleep 60", "chat_id": "timeout-test" }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["exit_code"], 124, "expected timeout exit code 124");
}

#[tokio::test]
async fn test_output_truncation() {
    let f = SandboxTestFixture::shared().await;

    // Generate >100KB of output
    let resp = f
        .client
        .post(f.url("/execute/bash"))
        .json(&json!({
            "command": "python3 -c \"print('x' * 200000)\"",
            "chat_id": "truncation-test"
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    let stdout = body["stdout"].as_str().unwrap();
    assert!(
        stdout.len() < 110_000,
        "expected output truncated to ~100KB, got {} bytes",
        stdout.len()
    );
    assert!(
        stdout.ends_with("... (output truncated)"),
        "expected truncation marker"
    );
}

// ---------------------------------------------------------------------------
// File operation tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_file_write_read_roundtrip() {
    let f = SandboxTestFixture::shared().await;
    let chat_id = "file-roundtrip";

    // Write with nested path (parent dirs should be auto-created)
    let resp = f
        .client
        .post(f.url("/files/write"))
        .json(&json!({
            "path": "sub/dir/file.txt",
            "content": "hello world",
            "chat_id": chat_id
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // Read it back
    let resp = f
        .client
        .post(f.url("/files/read"))
        .json(&json!({ "path": "sub/dir/file.txt", "chat_id": chat_id }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["content"].as_str().unwrap(), "1 | hello world");
}

#[tokio::test]
async fn test_file_read_line_range() {
    let f = SandboxTestFixture::shared().await;
    let chat_id = "line-range";

    let content = "line1\nline2\nline3\nline4\nline5\n";
    f.client
        .post(f.url("/files/write"))
        .json(&json!({ "path": "lines.txt", "content": content, "chat_id": chat_id }))
        .send()
        .await
        .unwrap();

    let resp = f
        .client
        .post(f.url("/files/read"))
        .json(&json!({
            "path": "lines.txt",
            "chat_id": chat_id,
            "start_line": 2,
            "end_line": 4
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    let content = body["content"].as_str().unwrap();
    assert_eq!(content, "2 | line2\n3 | line3\n4 | line4");
}

#[tokio::test]
async fn test_file_edit_exact_match() {
    let f = SandboxTestFixture::shared().await;
    let chat_id = "file-edit-basic";

    f.client
        .post(f.url("/files/write"))
        .json(&json!({
            "path": "doc.md",
            "content": "# Title\n\nOld paragraph.\n\nAnother section.\n",
            "chat_id": chat_id
        }))
        .send()
        .await
        .unwrap();

    let resp = f
        .client
        .post(f.url("/files/edit"))
        .json(&json!({
            "path": "doc.md",
            "old_string": "Old paragraph.",
            "new_string": "New paragraph.",
            "chat_id": chat_id
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert!(body["content"].as_str().unwrap().contains("1 replacement"));

    let resp = f
        .client
        .post(f.url("/files/read"))
        .json(&json!({ "path": "doc.md", "chat_id": chat_id }))
        .send()
        .await
        .unwrap();
    let body: Value = resp.json().await.unwrap();
    let content = body["content"].as_str().unwrap();
    assert!(content.contains("New paragraph."));
    assert!(!content.contains("Old paragraph."));
    assert!(content.contains("Another section."));
}

#[tokio::test]
async fn test_file_edit_errors() {
    let f = SandboxTestFixture::shared().await;
    let chat_id = "file-edit-errors";

    let content = "repeat\nrepeat\nunique\n";
    f.client
        .post(f.url("/files/write"))
        .json(&json!({ "path": "doc.txt", "content": content, "chat_id": chat_id }))
        .send()
        .await
        .unwrap();

    // No match
    let resp = f
        .client
        .post(f.url("/files/edit"))
        .json(&json!({
            "path": "doc.txt",
            "old_string": "does not exist",
            "new_string": "x",
            "chat_id": chat_id
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);

    // Ambiguous match without replace_all
    let resp = f
        .client
        .post(f.url("/files/edit"))
        .json(&json!({
            "path": "doc.txt",
            "old_string": "repeat",
            "new_string": "x",
            "chat_id": chat_id
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    let body: Value = resp.json().await.unwrap();
    assert!(body["detail"].as_str().unwrap().contains("2 locations"));

    // Empty old_string
    let resp = f
        .client
        .post(f.url("/files/edit"))
        .json(&json!({
            "path": "doc.txt",
            "old_string": "",
            "new_string": "x",
            "chat_id": chat_id
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);

    // Missing file
    let resp = f
        .client
        .post(f.url("/files/edit"))
        .json(&json!({
            "path": "missing.txt",
            "old_string": "a",
            "new_string": "b",
            "chat_id": chat_id
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);

    // File should be unchanged after all failures
    let resp = f
        .client
        .post(f.url("/files/read"))
        .json(&json!({ "path": "doc.txt", "chat_id": chat_id }))
        .send()
        .await
        .unwrap();
    let body: Value = resp.json().await.unwrap();
    assert_eq!(
        body["content"].as_str().unwrap(),
        "1 | repeat\n2 | repeat\n3 | unique"
    );
}

#[tokio::test]
async fn test_file_edit_replace_all() {
    let f = SandboxTestFixture::shared().await;
    let chat_id = "file-edit-replace-all";

    f.client
        .post(f.url("/files/write"))
        .json(&json!({
            "path": "doc.txt",
            "content": "foo bar foo\nfoo\n",
            "chat_id": chat_id
        }))
        .send()
        .await
        .unwrap();

    let resp = f
        .client
        .post(f.url("/files/edit"))
        .json(&json!({
            "path": "doc.txt",
            "old_string": "foo",
            "new_string": "baz",
            "replace_all": true,
            "chat_id": chat_id
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert!(body["content"].as_str().unwrap().contains("3 replacements"));

    let resp = f
        .client
        .post(f.url("/files/read"))
        .json(&json!({ "path": "doc.txt", "chat_id": chat_id }))
        .send()
        .await
        .unwrap();
    let body: Value = resp.json().await.unwrap();
    assert_eq!(
        body["content"].as_str().unwrap(),
        "1 | baz bar baz\n2 | baz"
    );
}

#[tokio::test]
async fn test_file_edit_binary_rejected() {
    let f = SandboxTestFixture::shared().await;
    let chat_id = "file-edit-binary";

    let raw_bytes: Vec<u8> = vec![0x00, 0xFF, 0xFE, 0x01];
    f.client
        .post(f.url("/files/write_binary"))
        .json(&json!({
            "path": "data.bin",
            "content_base64": BASE64.encode(&raw_bytes),
            "chat_id": chat_id
        }))
        .send()
        .await
        .unwrap();

    let resp = f
        .client
        .post(f.url("/files/edit"))
        .json(&json!({
            "path": "data.bin",
            "old_string": "x",
            "new_string": "y",
            "chat_id": chat_id
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    let body: Value = resp.json().await.unwrap();
    assert!(body["detail"].as_str().unwrap().contains("binary"));
}

#[tokio::test]
async fn test_binary_write_and_download() {
    let f = SandboxTestFixture::shared().await;
    let chat_id = "binary-test";

    let raw_bytes: Vec<u8> = vec![0x00, 0x01, 0x02, 0xFF, 0xFE, 0xFD];
    let b64 = BASE64.encode(&raw_bytes);

    f.client
        .post(f.url("/files/write_binary"))
        .json(&json!({ "path": "test.bin", "content_base64": b64, "chat_id": chat_id }))
        .send()
        .await
        .unwrap();

    let resp = f
        .client
        .get(f.url("/files/download"))
        .query(&[("path", "test.bin"), ("chat_id", chat_id)])
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.headers()
            .get("access-control-allow-origin")
            .and_then(|v| v.to_str().ok()),
        Some("*"),
    );
    let downloaded = resp.bytes().await.unwrap();
    assert_eq!(downloaded.as_ref(), &raw_bytes);
}

#[tokio::test]
async fn test_file_stat() {
    let f = SandboxTestFixture::shared().await;
    let chat_id = "stat-test";

    f.client
        .post(f.url("/files/write"))
        .json(&json!({ "path": "exists.txt", "content": "data", "chat_id": chat_id }))
        .send()
        .await
        .unwrap();

    // Existing file
    let resp = f
        .client
        .post(f.url("/files/stat"))
        .json(&json!({ "path": "exists.txt", "chat_id": chat_id }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["exists"], true);
    assert!(body["size_bytes"].as_u64().unwrap() > 0);

    // Missing file
    let resp = f
        .client
        .post(f.url("/files/stat"))
        .json(&json!({ "path": "nope.txt", "chat_id": chat_id }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["exists"], false);
}

// ---------------------------------------------------------------------------
// Versioning tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_versioning_write_edit_and_history() {
    let f = SandboxTestFixture::shared().await;
    let chat_id = "versioning-basic";

    f.client
        .post(f.url("/files/write"))
        .json(&json!({
            "path": "report.md",
            "content": "version one\n",
            "chat_id": chat_id
        }))
        .send()
        .await
        .unwrap();

    // stat exposes the version that present_artifact pins to
    let resp = f
        .client
        .post(f.url("/files/stat"))
        .json(&json!({ "path": "report.md", "chat_id": chat_id }))
        .send()
        .await
        .unwrap();
    let body: Value = resp.json().await.unwrap();
    let v1 = body["version"]
        .as_str()
        .expect("stat should expose version")
        .to_string();
    assert!(!v1.is_empty());

    // edit_file creates a second version
    f.client
        .post(f.url("/files/edit"))
        .json(&json!({
            "path": "report.md",
            "old_string": "version one",
            "new_string": "version two",
            "chat_id": chat_id
        }))
        .send()
        .await
        .unwrap();

    let resp = f
        .client
        .post(f.url("/files/versions"))
        .json(&json!({ "path": "report.md", "chat_id": chat_id }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let versions: Vec<Value> = resp.json().await.unwrap();
    assert_eq!(versions.len(), 2, "expected one commit per mutation");
    assert!(
        versions[0]["message"]
            .as_str()
            .unwrap()
            .starts_with("edit_file:")
    );
    assert!(
        versions[1]["message"]
            .as_str()
            .unwrap()
            .starts_with("write_file:")
    );
    assert!(versions[1]["timestamp"].as_str().is_some());
    assert_ne!(versions[0]["sha"], versions[1]["sha"]);

    // The version-1 sha still serves the original content (version pinning)
    let resp = f
        .client
        .get(f.url("/files/download"))
        .query(&[
            ("path", "report.md"),
            ("chat_id", chat_id),
            ("version", v1.as_str()),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let bytes = resp.bytes().await.unwrap();
    assert_eq!(bytes.as_ref(), b"version one\n");

    // Default download keeps serving the latest content
    let resp = f
        .client
        .get(f.url("/files/download"))
        .query(&[("path", "report.md"), ("chat_id", chat_id)])
        .send()
        .await
        .unwrap();
    let bytes = resp.bytes().await.unwrap();
    assert_eq!(bytes.as_ref(), b"version two\n");
}

#[tokio::test]
async fn test_versioning_bash_writes_are_committed() {
    let f = SandboxTestFixture::shared().await;
    let chat_id = "versioning-bash";

    f.client
        .post(f.url("/execute/bash"))
        .json(&json!({
            "command": "echo from-bash > out.txt",
            "chat_id": chat_id
        }))
        .send()
        .await
        .unwrap();

    let resp = f
        .client
        .post(f.url("/files/versions"))
        .json(&json!({ "path": "out.txt", "chat_id": chat_id }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let versions: Vec<Value> = resp.json().await.unwrap();
    assert!(!versions.is_empty(), "bash mutations should be committed");
    assert!(
        versions[0]["message"]
            .as_str()
            .unwrap()
            .starts_with("run_bash")
    );
}

#[tokio::test]
async fn test_versioning_binary_content_roundtrip() {
    let f = SandboxTestFixture::shared().await;
    let chat_id = "versioning-binary";

    let raw_bytes: Vec<u8> = vec![0x00, 0xFF, 0xFE, 0x01, 0x02];
    f.client
        .post(f.url("/files/write_binary"))
        .json(&json!({
            "path": "file.bin",
            "content_base64": BASE64.encode(&raw_bytes),
            "chat_id": chat_id
        }))
        .send()
        .await
        .unwrap();

    let resp = f
        .client
        .post(f.url("/files/stat"))
        .json(&json!({ "path": "file.bin", "chat_id": chat_id }))
        .send()
        .await
        .unwrap();
    let body: Value = resp.json().await.unwrap();
    let v1 = body["version"].as_str().unwrap().to_string();

    let resp = f
        .client
        .get(f.url("/files/download"))
        .query(&[
            ("path", "file.bin"),
            ("chat_id", chat_id),
            ("version", v1.as_str()),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let bytes = resp.bytes().await.unwrap();
    assert_eq!(bytes.as_ref(), &raw_bytes[..]);
}

#[tokio::test]
async fn test_versioning_errors() {
    let f = SandboxTestFixture::shared().await;
    let chat_id = "versioning-errors";

    // No history at all for an untouched chat
    let resp = f
        .client
        .post(f.url("/files/versions"))
        .json(&json!({ "path": "nope.txt", "chat_id": chat_id }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);

    f.client
        .post(f.url("/files/write"))
        .json(&json!({ "path": "a.txt", "content": "x", "chat_id": chat_id }))
        .send()
        .await
        .unwrap();

    // Unknown version ref
    let resp = f
        .client
        .get(f.url("/files/download"))
        .query(&[
            ("path", "a.txt"),
            ("chat_id", chat_id),
            ("version", "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef"),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);

    // Non-hex ref is rejected
    let resp = f
        .client
        .get(f.url("/files/download"))
        .query(&[
            ("path", "a.txt"),
            ("chat_id", chat_id),
            ("version", "; rm -rf"),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
}

// ---------------------------------------------------------------------------
// Security & isolation tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_path_traversal_blocked() {
    let f = SandboxTestFixture::shared().await;

    let resp = f
        .client
        .post(f.url("/files/write"))
        .json(&json!({
            "path": "../../../etc/passwd",
            "content": "hacked",
            "chat_id": "traversal-test"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);

    let resp = f
        .client
        .post(f.url("/files/read"))
        .json(&json!({
            "path": "../../../etc/passwd",
            "chat_id": "traversal-test"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn test_chat_isolation() {
    let f = SandboxTestFixture::shared().await;

    // Write a file in chat-a
    f.client
        .post(f.url("/files/write"))
        .json(&json!({
            "path": "secret.txt",
            "content": "chat-a-data",
            "chat_id": "chat-a"
        }))
        .send()
        .await
        .unwrap();

    // Try to read it from chat-b
    let resp = f
        .client
        .post(f.url("/files/read"))
        .json(&json!({ "path": "secret.txt", "chat_id": "chat-b" }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        404,
        "chat-b should not be able to read chat-a's file"
    );
}

#[tokio::test]
async fn test_landlock_blocks_write_to_readonly_paths() {
    let f = SandboxTestFixture::shared().await;

    let resp = f
        .client
        .post(f.url("/execute/bash"))
        .json(&json!({
            "command": "touch /usr/test_file",
            "chat_id": "landlock-write"
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_ne!(
        body["exit_code"], 0,
        "writing to /usr should fail under Landlock"
    );
}

#[tokio::test]
async fn test_landlock_blocks_read_outside_allowed() {
    let f = SandboxTestFixture::shared().await;

    let resp = f
        .client
        .post(f.url("/execute/bash"))
        .json(&json!({
            "command": "cat /root/.bashrc",
            "chat_id": "landlock-read"
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_ne!(
        body["exit_code"], 0,
        "reading from /root should fail under Landlock"
    );
}

#[tokio::test]
async fn test_landlock_allows_read_only_paths() {
    let f = SandboxTestFixture::shared().await;

    let resp = f
        .client
        .post(f.url("/execute/bash"))
        .json(&json!({
            "command": "ls /usr/bin > /dev/null",
            "chat_id": "landlock-readonly"
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(
        body["exit_code"], 0,
        "reading /usr/bin should succeed under Landlock"
    );
}

#[tokio::test]
async fn test_chat_dir_write_works() {
    let f = SandboxTestFixture::shared().await;

    let resp = f
        .client
        .post(f.url("/execute/bash"))
        .json(&json!({
            "command": "echo test > myfile && cat myfile",
            "chat_id": "landlock-chatdir"
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["exit_code"], 0);
    assert_eq!(body["stdout"].as_str().unwrap().trim(), "test");
}
