use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    searcher::run_server().await
}
