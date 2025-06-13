use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    clio_searcher::run_server().await
}
