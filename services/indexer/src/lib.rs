pub mod error;
pub mod queue_processor;

pub use error::{IndexerError, Result};
pub use queue_processor::QueueProcessor;
pub use shared::models::{ConnectorEvent, DocumentMetadata, DocumentPermissions};

pub use axum::Router;
pub use redis::Client as RedisClient;
pub use serde::{Deserialize, Serialize};
pub use serde_json::Value;
pub use shared::db::pool::DatabasePool;
pub use shared::AIClient;
use shared::ServiceCredentialsRepo;
use std::sync::Arc;

use axum::{
    extract::{Path, State},
    middleware,
    response::Json,
    routing::{delete, get, post, put},
};
use error::Result as IndexerResult;
use serde_json::json;
use shared::{
    db::repositories::{DocumentRepository, OrphanStats},
    models::Document,
    storage::gc::{ContentBlobGC, GCConfig, GCResult},
    telemetry::{self, TelemetryConfig},
    IndexerConfig,
};
use sqlx::types::time::OffsetDateTime;
use std::net::SocketAddr;
use tower::ServiceBuilder;
use tower_http::cors::CorsLayer;
use tracing::{error, info};
use ulid::Ulid;

#[derive(Clone)]
pub struct AppState {
    pub db_pool: DatabasePool,
    pub redis_client: RedisClient,
    pub ai_client: AIClient,
    pub content_storage: Arc<dyn shared::ObjectStorage>,
    pub embedding_queue: shared::embedding_queue::EmbeddingQueue,
    pub service_credentials_repo: Arc<ServiceCredentialsRepo>,
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

#[derive(Debug, Deserialize, Serialize)]
pub struct CreateServiceCredentialsRequest {
    pub source_id: String,
    pub provider: shared::models::ServiceProvider,
    pub auth_type: shared::models::AuthType,
    pub principal_email: Option<String>,
    pub credentials: Value,
    pub config: Value,
}

#[derive(Debug, Serialize)]
pub struct CreateServiceCredentialsResponse {
    pub success: bool,
    pub message: String,
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
        .route("/service-credentials", post(create_service_credentials))
        .route("/admin/gc/run", post(run_gc))
        .route("/admin/gc/stats", get(gc_stats))
        .layer(
            ServiceBuilder::new()
                .layer(middleware::from_fn(telemetry::middleware::trace_layer))
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
) -> IndexerResult<Json<Document>> {
    let document_id = Ulid::new().to_string();
    let now = OffsetDateTime::now_utc();

    let content_id = state
        .content_storage
        .store_content_with_type(request.content.as_bytes(), Some("text/plain"), None)
        .await
        .map_err(|e| error::IndexerError::Internal(format!("Failed to store content: {}", e)))?;

    let doc = Document {
        id: document_id.clone(),
        source_id: request.source_id,
        external_id: request.external_id,
        title: request.title,
        content_id: Some(content_id),
        content_type: Some("text/plain".to_string()),
        file_size: None,
        file_extension: None,
        url: None,
        metadata: request.metadata,
        permissions: request.permissions,
        attributes: serde_json::json!({}),
        created_at: now,
        updated_at: now,
        last_indexed_at: now,
    };

    let repo = DocumentRepository::new(state.db_pool.pool());
    let document = repo.create(doc).await?;

    info!("Created document: {}", document_id);
    Ok(Json(document))
}

async fn get_document(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> IndexerResult<Json<Document>> {
    let repo = DocumentRepository::new(state.db_pool.pool());
    match repo.find_by_id(&id).await? {
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
) -> IndexerResult<Json<Document>> {
    let content_id = if let Some(content) = &request.content {
        let cid = state
            .content_storage
            .store_content_with_type(content.as_bytes(), Some("text/plain"), None)
            .await
            .map_err(|e| {
                error::IndexerError::Internal(format!("Failed to store content: {}", e))
            })?;
        Some(cid)
    } else {
        None
    };

    let repo = DocumentRepository::new(state.db_pool.pool());
    let updated_doc = repo
        .update_fields(
            &id,
            request.title.as_deref(),
            content_id.as_deref(),
            request.metadata.as_ref(),
            request.permissions.as_ref(),
        )
        .await?;

    match updated_doc {
        Some(doc) => {
            info!("Updated document: {}", id);
            Ok(Json(doc))
        }
        None => Err(error::IndexerError::NotFound(format!(
            "Document {} not found",
            id
        ))),
    }
}

async fn delete_document(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> IndexerResult<Json<Value>> {
    let repo = DocumentRepository::new(state.db_pool.pool());
    let deleted = repo.delete(&id).await?;

    if !deleted {
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

    let content_id = state
        .content_storage
        .store_content_with_type(request.content.as_bytes(), Some("text/plain"), None)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to store content: {}", e))?;

    let doc = Document {
        id: document_id,
        source_id: request.source_id,
        external_id: request.external_id,
        title: request.title,
        content_id: Some(content_id),
        content_type: Some("text/plain".to_string()),
        file_size: None,
        file_extension: None,
        url: None,
        metadata: request.metadata,
        permissions: request.permissions,
        attributes: serde_json::json!({}),
        created_at: now,
        updated_at: now,
        last_indexed_at: now,
    };

    let repo = DocumentRepository::new(state.db_pool.pool());
    repo.create(doc).await?;

    Ok(())
}

async fn process_update_operation(
    state: &AppState,
    id: String,
    request: UpdateDocumentRequest,
) -> anyhow::Result<()> {
    let content_id = if let Some(content) = &request.content {
        let cid = state
            .content_storage
            .store_content_with_type(content.as_bytes(), Some("text/plain"), None)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to store content: {}", e))?;
        Some(cid)
    } else {
        None
    };

    let repo = DocumentRepository::new(state.db_pool.pool());
    let updated = repo
        .update_fields(
            &id,
            request.title.as_deref(),
            content_id.as_deref(),
            request.metadata.as_ref(),
            request.permissions.as_ref(),
        )
        .await?;

    if updated.is_none() {
        return Err(anyhow::anyhow!("Document {} not found", id));
    }

    Ok(())
}

async fn process_delete_operation(state: &AppState, id: String) -> anyhow::Result<()> {
    let repo = DocumentRepository::new(state.db_pool.pool());
    let deleted = repo.delete(&id).await?;

    if !deleted {
        return Err(anyhow::anyhow!("Document {} not found", id));
    }

    Ok(())
}

async fn create_service_credentials(
    State(state): State<AppState>,
    Json(request): Json<CreateServiceCredentialsRequest>,
) -> IndexerResult<Json<CreateServiceCredentialsResponse>> {
    let service_credentials = shared::models::ServiceCredentials {
        id: Ulid::new().to_string(),
        source_id: request.source_id.clone(),
        provider: request.provider,
        auth_type: request.auth_type,
        principal_email: request.principal_email,
        credentials: request.credentials,
        config: request.config,
        expires_at: None,
        last_validated_at: None,
        created_at: OffsetDateTime::now_utc(),
        updated_at: OffsetDateTime::now_utc(),
    };

    // Delete existing credentials for this source first
    match state
        .service_credentials_repo
        .delete_by_source_id(&request.source_id)
        .await
    {
        Ok(_) => info!(
            "Deleted existing credentials for source: {}",
            request.source_id
        ),
        Err(e) => info!(
            "No existing credentials to delete for source {}: {}",
            request.source_id, e
        ),
    }

    // Create new credentials (this will automatically encrypt them)
    match state
        .service_credentials_repo
        .create(service_credentials)
        .await
    {
        Ok(_) => {
            info!(
                "Created encrypted service credentials for source: {}",
                request.source_id
            );
            Ok(Json(CreateServiceCredentialsResponse {
                success: true,
                message: "Service credentials created successfully".to_string(),
            }))
        }
        Err(e) => {
            error!("Failed to create service credentials: {}", e);
            Ok(Json(CreateServiceCredentialsResponse {
                success: false,
                message: format!("Failed to create service credentials: {}", e),
            }))
        }
    }
}

async fn run_gc(State(state): State<AppState>) -> IndexerResult<Json<GCResult>> {
    let gc = ContentBlobGC::new(
        state.db_pool.pool().clone(),
        state.content_storage.clone(),
        GCConfig::from_env(),
    );

    let result = gc
        .run()
        .await
        .map_err(|e| IndexerError::Internal(format!("GC failed: {}", e)))?;

    Ok(Json(result))
}

async fn gc_stats(State(state): State<AppState>) -> IndexerResult<Json<OrphanStats>> {
    let gc = ContentBlobGC::new(
        state.db_pool.pool().clone(),
        state.content_storage.clone(),
        GCConfig::from_env(),
    );

    let stats = gc
        .get_orphan_stats()
        .await
        .map_err(|e| IndexerError::Internal(format!("Failed to get GC stats: {}", e)))?;

    Ok(Json(stats))
}

pub async fn run_server() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();

    let telemetry_config = TelemetryConfig::from_env("omni-indexer");
    telemetry::init_telemetry(telemetry_config)?;

    info!("Indexer service starting...");

    let config = IndexerConfig::from_env();

    let db_pool = DatabasePool::from_config(&config.database)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to create database pool: {}", e))?;

    // Migrations are now handled by a separate migrator container
    info!("Database migrations handled by migrator container");

    let redis_client = RedisClient::open(config.redis.redis_url)?;
    info!("Redis client initialized");

    let ai_client = AIClient::new(config.ai_service_url.clone());
    info!("AI client initialized");

    let embedding_queue = shared::embedding_queue::EmbeddingQueue::new(db_pool.pool().clone());
    info!("Embedding queue initialized");

    let service_credentials_repo = Arc::new(ServiceCredentialsRepo::new(db_pool.pool().clone())?);
    info!("Service credentials repository initialized");

    let content_storage = shared::StorageFactory::from_env(db_pool.pool().clone()).await?;
    info!("Content storage initialized");

    let app_state = AppState {
        db_pool,
        redis_client,
        ai_client,
        content_storage,
        embedding_queue,
        service_credentials_repo,
    };

    let app = create_app(app_state.clone());

    let queue_processor = queue_processor::QueueProcessor::new(app_state.clone());
    let processor_handle = tokio::spawn(async move {
        if let Err(e) = queue_processor.start().await {
            error!("Queue processor failed: {}", e);
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
