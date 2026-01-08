use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    omni_connector_manager::run_server().await
}
