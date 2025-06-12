use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    indexer::run_server().await
}
