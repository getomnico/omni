pub mod error;
pub mod events;
pub mod processor;

pub use error::{IndexerError, Result};
pub use events::{ConnectorEvent, DocumentMetadata, DocumentPermissions};
pub use processor::EventProcessor;

pub use axum::Router;
pub use redis::Client as RedisClient;
pub use serde::{Deserialize, Serialize};
pub use serde_json::Value;
pub use shared::db::pool::DatabasePool;

use axum::{
    extract::{Path, State},
    response::Json,
    routing::{delete, get, post, put},
};
use error::Result as IndexerResult;
use serde_json::json;
use shared::IndexerConfig;
use sqlx::{types::time::OffsetDateTime, PgPool};
use std::net::SocketAddr;
use tower::ServiceBuilder;
use tower_http::{cors::CorsLayer, trace::TraceLayer};
use tracing::{error, info};
use ulid::Ulid;

#[derive(Clone)]
pub struct AppState {
    pub db_pool: DatabasePool,
    pub redis_client: RedisClient,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct CreateDocumentRequest {
    pub source_id: String,
    pub external_id: String,
    pub title: String,
    pub content: String,
    pub metadata: Value,
    pub permissions: Value,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct UpdateDocumentRequest {
    pub title: Option<String>,
    pub content: Option<String>,
    pub metadata: Option<Value>,
    pub permissions: Option<Value>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct BulkDocumentOperation {
    pub operation: String,
    pub document_id: Option<String>,
    pub document: Option<CreateDocumentRequest>,
    pub updates: Option<UpdateDocumentRequest>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct BulkDocumentRequest {
    pub operations: Vec<BulkDocumentOperation>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BulkDocumentResponse {
    pub success_count: usize,
    pub error_count: usize,
    pub errors: Vec<String>,
}

pub fn create_app(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health_check))
        .route("/debug", post(debug_create_document))
        .route("/documents", post(create_document))
        .route("/documents/bulk", post(bulk_documents))
        .route("/documents/:id", get(get_document))
        .route("/documents/:id", put(update_document))
        .route("/documents/:id", delete(delete_document))
        .layer(
            ServiceBuilder::new()
                .layer(TraceLayer::new_for_http())
                .layer(CorsLayer::permissive()),
        )
        .with_state(state)
}

async fn health_check(State(state): State<AppState>) -> IndexerResult<Json<Value>> {
    sqlx::query("SELECT 1")
        .execute(state.db_pool.pool())
        .await?;

    let mut redis_conn = state
        .redis_client
        .get_multiplexed_async_connection()
        .await?;
    redis::cmd("PING")
        .query_async::<String>(&mut redis_conn)
        .await?;

    Ok(Json(json!({
        "status": "healthy",
        "service": "indexer",
        "database": "connected",
        "redis": "connected",
        "timestamp": OffsetDateTime::now_utc().to_string()
    })))
}

async fn create_document(
    State(state): State<AppState>,
    Json(request): Json<CreateDocumentRequest>,
) -> IndexerResult<Json<shared::models::Document>> {
    let document_id = Ulid::new().to_string();
    let now = OffsetDateTime::now_utc();

    let document = sqlx::query_as::<_, shared::models::Document>(
        r#"
        INSERT INTO documents (id, source_id, external_id, title, content, metadata, permissions, created_at, updated_at, last_indexed_at)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
        RETURNING *
        "#,
    )
    .bind(&document_id)
    .bind(&request.source_id)
    .bind(&request.external_id)
    .bind(&request.title)
    .bind(Some(&request.content))
    .bind(&request.metadata)
    .bind(&request.permissions)
    .bind(now)
    .bind(now)
    .bind(now)
    .fetch_one(state.db_pool.pool())
    .await?;

    info!("Created document: {}", document_id);
    Ok(Json(document))
}

async fn get_document(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> IndexerResult<Json<shared::models::Document>> {
    let document =
        sqlx::query_as::<_, shared::models::Document>("SELECT * FROM documents WHERE id = $1")
            .bind(&id)
            .fetch_optional(state.db_pool.pool())
            .await?;

    match document {
        Some(doc) => Ok(Json(doc)),
        None => Err(error::IndexerError::NotFound(format!(
            "Document {} not found",
            id
        ))),
    }
}

async fn update_document(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<UpdateDocumentRequest>,
) -> IndexerResult<Json<shared::models::Document>> {
    let existing_doc =
        sqlx::query_as::<_, shared::models::Document>("SELECT * FROM documents WHERE id = $1")
            .bind(&id)
            .fetch_optional(state.db_pool.pool())
            .await?;

    let _existing_doc = match existing_doc {
        Some(doc) => doc,
        None => {
            return Err(error::IndexerError::NotFound(format!(
                "Document {} not found",
                id
            )))
        }
    };

    let updated_doc = sqlx::query_as::<_, shared::models::Document>(
        r#"
        UPDATE documents 
        SET title = COALESCE($2, title),
            content = COALESCE($3, content),
            metadata = COALESCE($4, metadata),
            permissions = COALESCE($5, permissions),
            updated_at = $6
        WHERE id = $1
        RETURNING *
        "#,
    )
    .bind(&id)
    .bind(&request.title)
    .bind(&request.content)
    .bind(&request.metadata)
    .bind(&request.permissions)
    .bind(OffsetDateTime::now_utc())
    .fetch_one(state.db_pool.pool())
    .await?;

    info!("Updated document: {}", id);
    Ok(Json(updated_doc))
}

async fn delete_document(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> IndexerResult<Json<Value>> {
    let result = sqlx::query("DELETE FROM documents WHERE id = $1")
        .bind(&id)
        .execute(state.db_pool.pool())
        .await?;

    if result.rows_affected() == 0 {
        return Err(error::IndexerError::NotFound(format!(
            "Document {} not found",
            id
        )));
    }

    info!("Deleted document: {}", id);
    Ok(Json(json!({
        "message": "Document deleted successfully",
        "id": id
    })))
}

async fn bulk_documents(
    State(state): State<AppState>,
    Json(request): Json<BulkDocumentRequest>,
) -> IndexerResult<Json<BulkDocumentResponse>> {
    let mut success_count = 0;
    let mut error_count = 0;
    let mut errors = Vec::new();

    for operation in request.operations {
        let result = match operation.operation.as_str() {
            "create" => {
                if let Some(document) = operation.document {
                    process_create_operation(&state, document).await
                } else {
                    Err(anyhow::anyhow!("Create operation missing document data"))
                }
            }
            "update" => {
                if let (Some(doc_id), Some(updates)) = (operation.document_id, operation.updates) {
                    process_update_operation(&state, doc_id, updates).await
                } else {
                    Err(anyhow::anyhow!(
                        "Update operation missing document_id or updates"
                    ))
                }
            }
            "delete" => {
                if let Some(doc_id) = operation.document_id {
                    process_delete_operation(&state, doc_id).await
                } else {
                    Err(anyhow::anyhow!("Delete operation missing document_id"))
                }
            }
            _ => Err(anyhow::anyhow!(
                "Unknown operation: {}",
                operation.operation
            )),
        };

        match result {
            Ok(_) => success_count += 1,
            Err(e) => {
                error_count += 1;
                errors.push(e.to_string());
            }
        }
    }

    info!(
        "Bulk operation completed: {} success, {} errors",
        success_count, error_count
    );

    Ok(Json(BulkDocumentResponse {
        success_count,
        error_count,
        errors,
    }))
}

async fn process_create_operation(
    state: &AppState,
    request: CreateDocumentRequest,
) -> anyhow::Result<()> {
    let document_id = Ulid::new().to_string();
    let now = OffsetDateTime::now_utc();

    sqlx::query(
        r#"
        INSERT INTO documents (id, source_id, external_id, title, content, metadata, permissions, created_at, updated_at, last_indexed_at)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
        "#,
    )
    .bind(&document_id)
    .bind(&request.source_id)
    .bind(&request.external_id)
    .bind(&request.title)
    .bind(Some(&request.content))
    .bind(&request.metadata)
    .bind(&request.permissions)
    .bind(now)
    .bind(now)
    .bind(now)
    .execute(state.db_pool.pool())
    .await?;

    Ok(())
}

async fn process_update_operation(
    state: &AppState,
    id: String,
    request: UpdateDocumentRequest,
) -> anyhow::Result<()> {
    let result = sqlx::query(
        r#"
        UPDATE documents 
        SET title = COALESCE($2, title),
            content = COALESCE($3, content),
            metadata = COALESCE($4, metadata),
            permissions = COALESCE($5, permissions),
            updated_at = $6
        WHERE id = $1
        "#,
    )
    .bind(&id)
    .bind(&request.title)
    .bind(&request.content)
    .bind(&request.metadata)
    .bind(&request.permissions)
    .bind(OffsetDateTime::now_utc())
    .execute(state.db_pool.pool())
    .await?;

    if result.rows_affected() == 0 {
        return Err(anyhow::anyhow!("Document {} not found", id));
    }

    Ok(())
}

async fn process_delete_operation(state: &AppState, id: String) -> anyhow::Result<()> {
    let result = sqlx::query("DELETE FROM documents WHERE id = $1")
        .bind(&id)
        .execute(state.db_pool.pool())
        .await?;

    if result.rows_affected() == 0 {
        return Err(anyhow::anyhow!("Document {} not found", id));
    }

    Ok(())
}

pub async fn run_migrations(pool: &PgPool) -> anyhow::Result<()> {
    sqlx::migrate!("./migrations").run(pool).await?;
    Ok(())
}

pub async fn run_server() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();

    tracing_subscriber::fmt::init();

    info!("Indexer service starting...");

    let config = IndexerConfig::from_env();

    let db_pool = DatabasePool::from_config(&config.database)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to create database pool: {}", e))?;

    // TODO: Ideally this should be done outside of this service
    info!("Running database migrations...");
    match run_migrations(db_pool.pool()).await {
        Ok(_) => info!("Database migrations completed successfully"),
        Err(e) => {
            error!("Failed to run migrations: {}", e);
            return Err(e);
        }
    }

    let redis_client = RedisClient::open(config.redis.redis_url)?;
    info!("Redis client initialized");

    let app_state = AppState {
        db_pool,
        redis_client,
    };

    let app = create_app(app_state.clone());

    let processor = processor::EventProcessor::new(app_state.clone());
    let processor_handle = tokio::spawn(async move {
        if let Err(e) = processor.start().await {
            error!("Event processor failed: {}", e);
        }
    });

    let addr = SocketAddr::from(([0, 0, 0, 0], config.port));
    info!("Indexer service listening on {}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;

    tokio::select! {
        result = axum::serve(listener, app) => {
            if let Err(e) = result {
                error!("HTTP server failed: {}", e);
            }
        }
        _ = processor_handle => {
            error!("Event processor task completed unexpectedly");
        }
    }

    Ok(())
}

async fn debug_create_document(
    State(_state): State<AppState>,
    body: String,
) -> IndexerResult<Json<Value>> {
    info!("Raw request body: {}", body);
    info!("Body length: {}", body.len());

    match serde_json::from_str::<CreateDocumentRequest>(&body) {
        Ok(req) => {
            info!(
                "Successfully parsed request: source_id='{}' ({}), external_id='{}' ({})",
                req.source_id,
                req.source_id.len(),
                req.external_id,
                req.external_id.len()
            );
            Ok(Json(json!({"status": "parsed successfully"})))
        }
        Err(e) => {
            error!("Failed to parse request: {}", e);
            Ok(Json(json!({"error": format!("Parse error: {}", e)})))
        }
    }
}
