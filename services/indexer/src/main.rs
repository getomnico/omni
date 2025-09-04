use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    omni_indexer::run_server().await
}
