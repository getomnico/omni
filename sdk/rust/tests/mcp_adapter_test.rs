use std::collections::HashMap;
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use omni_connector_sdk::mcp_adapter::{HttpMcpServer, McpAdapter, McpServer, StdioMcpServer};

fn fixture_path() -> PathBuf {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    PathBuf::from(manifest_dir).join("../python/tests/test_mcp_server.py")
}

fn python_executable() -> String {
    // Use the python that lives in the SDK's uv-managed venv if present, else
    // fall back to plain `python` on PATH.
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let venv = PathBuf::from(manifest_dir).join("../python/.venv/bin/python");
    if venv.exists() {
        venv.to_string_lossy().into_owned()
    } else {
        "python3".to_string()
    }
}

fn stdio_server() -> StdioMcpServer {
    StdioMcpServer::new(python_executable())
        .with_args([fixture_path().to_string_lossy().into_owned()])
}

struct HttpFixture {
    child: Child,
    url: String,
}

impl HttpFixture {
    fn spawn() -> Self {
        let port = {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap().port()
        };
        let child = Command::new(python_executable())
            .arg(fixture_path())
            .arg("http")
            .arg(port.to_string())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("failed to spawn http MCP fixture");
        let url = format!("http://127.0.0.1:{}/mcp", port);

        // Poll until the TCP socket is open. We don't need to send a real
        // HTTP request — opening a connection means uvicorn is up.
        let addr = format!("127.0.0.1:{}", port);
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        loop {
            if std::time::Instant::now() >= deadline {
                panic!("HTTP fixture did not become ready at {}", url);
            }
            if TcpStream::connect_timeout(&addr.parse().unwrap(), Duration::from_millis(200))
                .is_ok()
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }

        Self { child, url }
    }

    fn server(&self) -> McpServer {
        McpServer::Http(HttpMcpServer::new(self.url.clone()))
    }
}

impl Drop for HttpFixture {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[tokio::test]
async fn stdio_lists_tools() {
    let adapter = McpAdapter::new(McpServer::Stdio(stdio_server()));
    let actions = adapter
        .get_action_definitions(Some(HashMap::new()), None)
        .await
        .expect("list_tools");
    let names: Vec<String> = actions.iter().map(|a| a.name.clone()).collect();
    assert!(names.contains(&"greet".to_string()));
    assert!(names.contains(&"add".to_string()));
    let greet = actions.iter().find(|a| a.name == "greet").unwrap();
    assert_eq!(greet.mode, "read");
    let add = actions.iter().find(|a| a.name == "add").unwrap();
    assert_eq!(add.mode, "write");
}

#[tokio::test]
async fn stdio_lists_resources_and_prompts() {
    let adapter = McpAdapter::new(McpServer::Stdio(stdio_server()));
    let resources = adapter
        .get_resource_definitions(Some(HashMap::new()), None)
        .await
        .unwrap();
    assert_eq!(resources.len(), 1);
    assert_eq!(resources[0].uri_template, "test://item/{item_id}");

    let prompts = adapter
        .get_prompt_definitions(Some(HashMap::new()), None)
        .await
        .unwrap();
    assert_eq!(prompts.len(), 1);
    assert_eq!(prompts[0].name, "summarize");
    assert!(prompts[0]
        .arguments
        .iter()
        .any(|a| a.name == "text" && a.required));
}

#[tokio::test]
async fn stdio_executes_tool() {
    let adapter = McpAdapter::new(McpServer::Stdio(stdio_server()));
    let response = adapter
        .execute_tool(
            "greet",
            serde_json::json!({ "name": "World" }),
            Some(HashMap::new()),
            None,
        )
        .await;
    assert_eq!(response.status, "success");
    let result = response.result.expect("result");
    assert!(result["content"]
        .as_str()
        .unwrap()
        .contains("Hello, World!"));
}

#[tokio::test]
async fn stdio_reads_resource_and_prompt() {
    let adapter = McpAdapter::new(McpServer::Stdio(stdio_server()));
    let resource = adapter
        .read_resource("test://item/42", Some(HashMap::new()), None)
        .await
        .unwrap();
    let contents = resource["contents"].as_array().unwrap();
    assert!(!contents.is_empty());

    let prompt = adapter
        .get_prompt(
            "summarize",
            Some(serde_json::json!({ "text": "hello world" })),
            Some(HashMap::new()),
            None,
        )
        .await
        .unwrap();
    let messages = prompt["messages"].as_array().unwrap();
    assert!(!messages.is_empty());
    assert!(messages[0]["content"]["text"]
        .as_str()
        .unwrap()
        .contains("hello world"));
}

#[tokio::test]
async fn stdio_caches_after_discover() {
    let adapter = McpAdapter::new(McpServer::Stdio(stdio_server()));
    adapter
        .discover(Some(HashMap::new()), None)
        .await
        .expect("discover");
    let actions = adapter.get_action_definitions(None, None).await.unwrap();
    assert_eq!(actions.len(), 2);
    let resources = adapter.get_resource_definitions(None, None).await.unwrap();
    assert_eq!(resources.len(), 1);
    let prompts = adapter.get_prompt_definitions(None, None).await.unwrap();
    assert_eq!(prompts.len(), 1);
}

#[tokio::test]
async fn http_lists_tools_and_executes() {
    let fixture = HttpFixture::spawn();
    let adapter = McpAdapter::new(fixture.server());
    let mut headers = HashMap::new();
    headers.insert("X-Test".to_string(), "1".to_string());

    let actions = adapter
        .get_action_definitions(None, Some(headers.clone()))
        .await
        .unwrap();
    let names: Vec<&str> = actions.iter().map(|a| a.name.as_str()).collect();
    assert!(names.contains(&"greet"));
    assert!(names.contains(&"add"));

    let response = adapter
        .execute_tool(
            "greet",
            serde_json::json!({ "name": "Remote" }),
            None,
            Some(headers),
        )
        .await;
    assert_eq!(response.status, "success");
    assert!(response
        .result
        .unwrap()
        .get("content")
        .and_then(|v| v.as_str())
        .unwrap()
        .contains("Hello, Remote!"));
}

#[tokio::test]
async fn http_caches_after_discover() {
    let fixture = HttpFixture::spawn();
    let adapter = McpAdapter::new(fixture.server());
    let mut headers = HashMap::new();
    headers.insert("X-Test".to_string(), "1".to_string());

    adapter
        .discover(None, Some(headers))
        .await
        .expect("discover");
    let actions = adapter.get_action_definitions(None, None).await.unwrap();
    assert_eq!(actions.len(), 2);
}

#[tokio::test]
async fn no_auth_no_cache_returns_empty() {
    let adapter = McpAdapter::new(McpServer::Stdio(stdio_server()));
    assert!(adapter
        .get_action_definitions(None, None)
        .await
        .unwrap()
        .is_empty());
    assert!(adapter
        .get_resource_definitions(None, None)
        .await
        .unwrap()
        .is_empty());
    assert!(adapter
        .get_prompt_definitions(None, None)
        .await
        .unwrap()
        .is_empty());
}
