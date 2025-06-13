use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    clio_indexer::run_server().await
}
