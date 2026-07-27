//! E2E cross-service trace continuity test.
//!
//! 1. Tags pre-built local dev images as production-style e2e images.
//! 2. Builds and starts a mock OTLP collector as a compose service.
//! 3. Starts the Omni stack pointing at the collector with JSON protocol.
//! 4. Triggers a search via `docker exec` inside the web container.
//! 5. Inspects captured spans from the mock collector via `docker exec`.
//! 6. Asserts cross-service trace continuity.
//!
//! Requires Docker. Skips gracefully when unavailable.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::Deserialize;

const E2E_TAG: &str = "e2e";

// ─────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────

fn docker_available() -> bool {
    Command::new("docker")
        .arg("info")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("shared is not inside a workspace")
        .to_path_buf()
}

const CORE_SERVICES: &[(&str, &str)] = &[
    ("searcher", "omni-searcher"),
    ("indexer", "omni-indexer"),
    ("ai", "omni-ai"),
    ("connector-manager", "omni-connector-manager"),
    ("web", "omni-web"),
    ("migrator", "omni-migrator"),
    ("sandbox", "omni-sandbox"),
];

fn prod_image_name(svc: &str) -> String {
    format!("ghcr.io/getomnico/omni/{svc}:{E2E_TAG}")
}

fn dev_image_name(svc: &str) -> String {
    format!("omni-{svc}:dev")
}

// ─────────────────────────────────────────────────────────
// Inspect types
// ─────────────────────────────────────────────────────────

#[derive(Clone, Debug, Deserialize)]
struct SpanSummary {
    trace_id: String,
    span_id: String,
    parent_span_id: String,
    name: String,
    kind: i32,
    service_name: String,
    #[allow(dead_code)]
    status_code: i32,
    #[allow(dead_code)]
    attributes: Vec<(String, String)>,
    #[allow(dead_code)]
    links: Vec<(String, String)>,
}

#[derive(Clone, Debug, Deserialize)]
#[allow(dead_code)]
struct InspectResponse {
    spans: Vec<SpanSummary>,
    service_names: Vec<String>,
    metric_names: Vec<String>,
}

// ─────────────────────────────────────────────────────────
// Compose orchestration
// ─────────────────────────────────────────────────────────

struct ComposeStack;

impl ComposeStack {
    fn env_file() -> PathBuf {
        let root = workspace_root();
        let dot_env = root.join(".env");
        if dot_env.exists() {
            dot_env
        } else {
            root.join(".env.example")
        }
    }

    fn tag_images() {
        for (svc_name, full_name) in CORE_SERVICES {
            let tag_status = Command::new("docker")
                .args([
                    "tag",
                    &dev_image_name(svc_name),
                    &prod_image_name(full_name),
                ])
                .stdout(Stdio::null())
                .stderr(Stdio::inherit())
                .status()
                .expect("docker tag failed");
            assert!(tag_status.success(), "Failed to tag {full_name}");
        }
    }

    fn build_mock_collector() -> String {
        let tag = "omni-mock-collector:e2e";
        let tmp_dir = std::env::temp_dir().join("omni-e2e-collector");
        let _ = std::fs::create_dir_all(&tmp_dir);

        // Node.js collector that parses JSON OTLP (Node exports) and protobuf OTLP (Rust exports)
        std::fs::write(
            tmp_dir.join("server.mjs"),
            include_str!("collector/server.mjs"),
        )
        .expect("failed to write collector server.mjs");

        std::fs::write(
            tmp_dir.join("package.json"),
            r#"{"dependencies":{"protobufjs":"^7.4.0"}}"#,
        )
        .expect("failed to write package.json");

        std::fs::write(tmp_dir.join("Dockerfile"),
            "FROM node:22-alpine\nWORKDIR /app\nCOPY package.json .\nRUN npm install --no-audit --no-fund 2>&1 | tail -1\nCOPY server.mjs .\nCMD [\"node\", \"server.mjs\"]\n")
            .expect("failed to write Dockerfile");

        let status = Command::new("docker")
            .args(["build", "-t", tag, "."])
            .current_dir(&tmp_dir)
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .expect("docker build failed");
        assert!(status.success(), "Failed to build mock collector");
        let _ = std::fs::remove_dir_all(&tmp_dir);
        tag.to_string()
    }

    async fn start() -> (String, PathBuf) {
        let project = format!("omni-e2e-{}", std::process::id());
        let tag = format!("e2e-{}", std::process::id());
        let collector_img = Self::build_mock_collector();

        let override_yaml = format!(
            r#"services:
  mock-collector:
    image: {collector_img}
    container_name: omni-{tag}-collector
    networks:
      - omni-network
  caddy:
    profiles: ["e2e-skip"]
  web-connector:
    profiles: ["e2e-skip"]
  postgres:
    container_name: omni-{tag}-postgres
  redis:
    container_name: omni-{tag}-redis
  migrator:
    container_name: omni-{tag}-migrator
  searcher:
    container_name: omni-{tag}-searcher
    environment:
      OTEL_EXPORTER_OTLP_PROTOCOL: http/json
  indexer:
    container_name: omni-{tag}-indexer
    environment:
      OTEL_EXPORTER_OTLP_PROTOCOL: http/json
  ai:
    container_name: omni-{tag}-ai
    environment:
      OTEL_EXPORTER_OTLP_PROTOCOL: http/json
  sandbox:
    container_name: omni-{tag}-sandbox
  connector-manager:
    container_name: omni-{tag}-connector-manager
    environment:
      OTEL_EXPORTER_OTLP_PROTOCOL: http/json
  web:
    container_name: omni-{tag}-web
    environment:
      OTEL_EXPORTER_OTLP_PROTOCOL: http/json
volumes:
  postgres_data:
    name: omni-{tag}-postgres-data
  redis_data:
    name: omni-{tag}-redis-data
  ai_models:
    name: omni-{tag}-ai-models
  sandbox_data:
    name: omni-{tag}-sandbox-data
"#,
        );
        let tmp = std::env::temp_dir().join(format!("omni-e2e-{}.yml", std::process::id()));
        std::fs::write(&tmp, &override_yaml).expect("failed to write temp compose override");

        let root = workspace_root();
        let compose_yml = root.join("docker/docker-compose.yml");
        let env_file = Self::env_file();

        let status = Command::new("docker")
            .args([
                "compose",
                "--env-file",
                env_file.to_str().unwrap(),
                "-f",
                compose_yml.to_str().unwrap(),
                "-f",
                tmp.to_str().unwrap(),
                "-p",
                &project,
                "up",
                "-d",
                "--pull",
                "never",
                "mock-collector",
                "postgres",
                "redis",
                "migrator",
                "searcher",
                "indexer",
                "ai",
                "connector-manager",
                "sandbox",
                "web",
            ])
            .env("OMNI_VERSION", E2E_TAG)
            .env("OTEL_EXPORTER_OTLP_ENDPOINT", "http://mock-collector:4318")
            .env("OTEL_DEPLOYMENT_ID", "e2e-test")
            .env("OTEL_METRIC_EXPORT_INTERVAL", "5000")
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .expect("docker compose up failed");
        assert!(status.success(), "docker compose up failed");

        // Wait for collector
        let coll_cnt = format!("omni-{tag}-collector");
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if Instant::now() > deadline {
                panic!("Collector not ready");
            }
            if Command::new("docker")
                .args([
                    "exec",
                    &coll_cnt,
                    "node",
                    "-e",
                    "fetch('http://localhost:4318/inspect')",
                ])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map_or(false, |s| s.success())
            {
                break;
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }

        // Wait for web health
        let web_cnt = format!("omni-{tag}-web");
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            if Instant::now() > deadline {
                panic!("Services not healthy");
            }
            if Command::new("docker")
                .args(["exec", &web_cnt, "node", "-e",
                       "fetch('http://localhost:5173/api/v1/health').then(r=>r.ok&&process.exit(0)).catch(()=>process.exit(1))"])
                .stdout(Stdio::null()).stderr(Stdio::null())
                .status().map_or(false, |s| s.success()) { break; }
            tokio::time::sleep(Duration::from_secs(3)).await;
        }

        (project, tmp)
    }

    fn down(project: &str, tmp: &PathBuf) {
        let _ = Command::new("docker")
            .args(["compose", "-p", project, "down", "-t", "30"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        for (_, full_name) in CORE_SERVICES {
            let _ = Command::new("docker")
                .args(["rmi", "--force", &prod_image_name(full_name)])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
        let _ = std::fs::remove_file(tmp);
    }
}

// ─────────────────────────────────────────────────────────
// Assertion helpers
// ─────────────────────────────────────────────────────────

fn group_by_trace(spans: &[SpanSummary]) -> HashMap<String, Vec<&SpanSummary>> {
    let mut groups: HashMap<String, Vec<&SpanSummary>> = HashMap::new();
    for s in spans {
        groups.entry(s.trace_id.clone()).or_default().push(s);
    }
    groups
}

fn assert_trace_chain(spans: &[&SpanSummary], label: &str) {
    assert!(!spans.is_empty(), "{label}: no spans in trace");
    let trace_ids: HashSet<&str> = spans.iter().map(|s| s.trace_id.as_str()).collect();
    assert_eq!(
        trace_ids.len(),
        1,
        "{label}: all spans must share one trace_id"
    );
    let roots: Vec<&&SpanSummary> = spans
        .iter()
        .filter(|s| s.parent_span_id.is_empty())
        .collect();
    assert_eq!(roots.len(), 1, "{label}: expected exactly one root span");
    let span_ids: HashSet<&str> = spans.iter().map(|s| s.span_id.as_str()).collect();
    for s in spans {
        if !s.parent_span_id.is_empty() {
            assert!(
                span_ids.contains(s.parent_span_id.as_str()),
                "{label}: span `{}` parent_span_id {} not found",
                s.name,
                s.parent_span_id
            );
        }
    }
}

// ─────────────────────────────────────────────────────────
// Test
// ─────────────────────────────────────────────────────────

#[tokio::test]
async fn test_cross_service_tracing() {
    if !docker_available() {
        eprintln!("SKIP: Docker not available");
        return;
    }

    println!("Tagging dev images...");
    ComposeStack::tag_images();

    println!("Starting compose stack...");
    let (project, tmp_path) = ComposeStack::start().await;
    let _guard = TestGuard {
        project: project.clone(),
        tmp: tmp_path.clone(),
    };

    let tag = format!("e2e-{}", std::process::id());
    let coll = format!("omni-{tag}-collector");
    let web = format!("omni-{tag}-web");

    // Reset and trigger search
    let _ = Command::new("docker")
        .args([
            "exec",
            &coll,
            "node",
            "-e",
            "fetch('http://localhost:4318/reset',{method:'POST'})",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    let status = Command::new("docker")
        .args(["exec", &web, "node", "-e",
               "fetch('http://localhost:5173/api/search',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({query:'hello',mode:'fulltext'})}).then(r=>{if(!r.ok){console.error('FAIL',r.status);process.exit(1)}console.log('Search OK')})"])
        .stdout(Stdio::inherit()).stderr(Stdio::inherit())
        .status().expect("docker exec failed");
    assert!(status.success(), "Search failed");

    tokio::time::sleep(Duration::from_secs(10)).await;

    // Inspect
    let output = Command::new("docker")
        .args([
            "exec",
            &coll,
            "node",
            "-e",
            "fetch('http://localhost:4318/inspect').then(r=>r.text()).then(t=>console.log(t))",
        ])
        .output()
        .expect("inspect failed");
    assert!(output.status.success(), "Inspect failed");

    let inspect: InspectResponse = serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
        panic!(
            "JSON parse: {e}\nstdout: {}",
            String::from_utf8_lossy(&output.stdout)
        )
    });

    let spans = &inspect.spans;
    println!(
        "Captured {} spans across services: {:?}",
        spans.len(),
        inspect.service_names
    );
    for s in spans {
        println!(
            "  kind={} svc={:?} name={:?} trace={} parent={}",
            s.kind,
            s.service_name,
            s.name,
            &s.trace_id[..8.min(s.trace_id.len())],
            &s.parent_span_id[..8.min(s.parent_span_id.len())]
        );
    }

    // Assert cross-service coverage
    assert!(
        inspect.service_names.len() >= 2,
        "Expected ≥2 services, got {:?}",
        inspect.service_names
    );

    // Find the search trace — it's the one with a POST /search span
    let groups = group_by_trace(spans);
    let Some((_tid, best)) = groups.iter().find(|(_, v)| {
        v.iter()
            .any(|s| s.name.contains("/search") || s.name.contains("POST"))
    }) else {
        panic!("No trace with a search request");
    };
    println!("Search trace: {} spans", best.len());

    assert_trace_chain(best, "search trace");

    // Verify web and searcher both present
    let has_web = best.iter().any(|s| s.service_name.contains("web"));
    let has_searcher = best.iter().any(|s| s.service_name.contains("searcher"));
    assert!(has_web, "No web spans in search trace");
    assert!(has_searcher, "No searcher spans in search trace");

    // Verify parent/child chain: the searcher SERVER span should have
    // a parent (the web CLIENT span) in this trace.
    for s in best
        .iter()
        .filter(|s| s.service_name.contains("searcher") && s.kind == 2)
    {
        assert!(
            !s.parent_span_id.is_empty(),
            "Searcher SERVER span should have a web parent"
        );
    }

    println!("✓ Cross-service trace continuity verified");
}

struct TestGuard {
    project: String,
    tmp: PathBuf,
}

impl Drop for TestGuard {
    fn drop(&mut self) {
        ComposeStack::down(&self.project, &self.tmp);
    }
}
