use std::path::PathBuf;
use std::process::Command;
use std::sync::Once;

use testcontainers::core::{IntoContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};
use tokio::sync::OnceCell;

static BUILD_IMAGE: Once = Once::new();
static SHARED_CONTAINER: OnceCell<SharedContainer> = OnceCell::const_new();

fn workspace_root() -> PathBuf {
    let mut dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    while !dir.join("Cargo.lock").exists() {
        assert!(dir.pop(), "Could not find workspace root (Cargo.lock)");
    }
    dir
}

fn build_sandbox_image() {
    BUILD_IMAGE.call_once(|| {
        let root = workspace_root();
        let status = Command::new("docker")
            .args([
                "build",
                "-t",
                "omni-sandbox:test",
                "--build-arg",
                "BUILD_MODE=release",
                "-f",
                "services/sandbox/Dockerfile",
                ".",
            ])
            .current_dir(&root)
            .status()
            .expect("failed to run docker build");
        assert!(status.success(), "docker build failed");
    });
}

struct SharedContainer {
    base_url: String,
    _container: ContainerAsync<GenericImage>,
}

pub struct SandboxTestFixture {
    pub client: reqwest::Client,
    pub base_url: String,
}

async fn start_container(execution_timeout: u32) -> SharedContainer {
    build_sandbox_image();

    let container = GenericImage::new("omni-sandbox", "test")
        .with_exposed_port(8090.tcp())
        .with_wait_for(WaitFor::message_on_stdout("Listening on"))
        .with_env_var("SANDBOX_ENABLED", "true")
        .with_env_var("EXECUTION_TIMEOUT", execution_timeout.to_string())
        .start()
        .await
        .expect("failed to start sandbox container");

    let host_port = container
        .get_host_port_ipv4(8090)
        .await
        .expect("failed to get mapped port");

    SharedContainer {
        base_url: format!("http://127.0.0.1:{host_port}"),
        _container: container,
    }
}

impl SandboxTestFixture {
    pub async fn shared() -> Self {
        let container = SHARED_CONTAINER.get_or_init(|| start_container(30)).await;
        SandboxTestFixture {
            client: reqwest::Client::new(),
            base_url: container.base_url.clone(),
        }
    }

    pub async fn with_timeout(execution_timeout: u32) -> Self {
        let container = start_container(execution_timeout).await;
        // Leak the container so it lives for the duration of the test.
        // Only used by test_execution_timeout, so one leaked container is fine.
        let leaked = Box::leak(Box::new(container));
        SandboxTestFixture {
            client: reqwest::Client::new(),
            base_url: leaked.base_url.clone(),
        }
    }

    pub fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }
}
