mod error;

use anyhow::Result;
use axum::{
    extract::State,
    response::Json,
    routing::get,
    Router,
};
use error::Result as IndexerResult;
use redis::Client as RedisClient;
use serde_json::{json, Value};
use shared::db::pool::DatabasePool;
use sqlx::PgPool;
use std::{env, net::SocketAddr};
use tower::ServiceBuilder;
use tower_http::{cors::CorsLayer, trace::TraceLayer};
use tracing::{info, error};


#[derive(Clone)]
pub struct AppState {
    pub db_pool: DatabasePool,
    pub redis_client: RedisClient,
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    
    tracing_subscriber::fmt::init();
    
    info!("Indexer service starting...");
    
    let database_url = env::var("DATABASE_URL")
        .expect("DATABASE_URL must be set");
    let redis_url = env::var("REDIS_URL")
        .unwrap_or_else(|_| "redis://localhost:6379".to_string());
    let port = env::var("PORT")
        .unwrap_or_else(|_| "3001".to_string())
        .parse::<u16>()
        .expect("PORT must be a valid number");
    
    let db_pool = DatabasePool::new(&database_url).await
        .map_err(|e| anyhow::anyhow!("Failed to create database pool: {}", e))?;
    
    info!("Running database migrations...");
    match run_migrations(db_pool.pool()).await {
        Ok(_) => info!("Database migrations completed successfully"),
        Err(e) => {
            error!("Failed to run migrations: {}", e);
            return Err(e);
        }
    }
    
    let redis_client = RedisClient::open(redis_url)?;
    info!("Redis client initialized");
    
    let app_state = AppState {
        db_pool,
        redis_client,
    };
    
    let app = create_app(app_state);
    
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    info!("Indexer service listening on {}", addr);
    
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    
    Ok(())
}

fn create_app(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health_check))
        .layer(
            ServiceBuilder::new()
                .layer(TraceLayer::new_for_http())
                .layer(CorsLayer::permissive()),
        )
        .with_state(state)
}

async fn health_check(State(state): State<AppState>) -> IndexerResult<Json<Value>> {
    sqlx::query("SELECT 1").execute(state.db_pool.pool()).await?;
    
    let mut redis_conn = state.redis_client.get_multiplexed_async_connection().await?;
    redis::cmd("PING").query_async::<String>(&mut redis_conn).await?;
    
    Ok(Json(json!({
        "status": "healthy",
        "service": "indexer",
        "database": "connected",
        "redis": "connected",
        "timestamp": chrono::Utc::now().to_rfc3339()
    })))
}

async fn run_migrations(pool: &PgPool) -> Result<()> {
    sqlx::migrate!("./migrations").run(pool).await?;
    Ok(())
}