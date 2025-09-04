use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    omni_searcher::run_server().await
}
