mod common;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use serde_json::{json, Value};

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
