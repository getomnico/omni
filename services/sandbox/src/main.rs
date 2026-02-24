#[tokio::main]
async fn main() -> anyhow::Result<()> {
    omni_sandbox::run_server().await
}
