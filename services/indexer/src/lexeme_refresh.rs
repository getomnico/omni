use shared::DatabasePool;
use std::time::Duration;
use tokio::time;
use tracing::{error, info};

pub async fn start_lexeme_refresh_task(db_pool: DatabasePool) {
    let mut interval = time::interval(Duration::from_secs(3600)); // Refresh every hour

    loop {
        interval.tick().await;

        info!("Starting lexeme materialized view refresh");

        match refresh_lexemes(&db_pool).await {
            Ok(_) => info!("Successfully refreshed lexeme materialized view"),
            Err(e) => error!("Failed to refresh lexeme materialized view: {}", e),
        }
    }
}

async fn refresh_lexemes(db_pool: &DatabasePool) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT refresh_unique_lexemes()")
        .execute(db_pool.pool())
        .await?;

    Ok(())
}
