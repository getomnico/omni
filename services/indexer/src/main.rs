use anyhow::Result;
use refinery::embed_migrations;
use sqlx::PgPool;
use std::env;
use tracing::{info, error};

embed_migrations!("migrations");

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    
    tracing_subscriber::fmt::init();
    
    info!("Indexer service starting...");
    
    let database_url = env::var("DATABASE_URL")
        .expect("DATABASE_URL must be set");
    
    let pool = PgPool::connect(&database_url).await?;
    
    info!("Running database migrations...");
    match run_migrations(&pool).await {
        Ok(_) => info!("Database migrations completed successfully"),
        Err(e) => {
            error!("Failed to run migrations: {}", e);
            return Err(e);
        }
    }
    
    info!("Indexer service ready");
    
    Ok(())
}

async fn run_migrations(pool: &PgPool) -> Result<()> {
    let mut conn = pool.acquire().await?;
    migrations::runner().run_async(&mut *conn).await?;
    Ok(())
}