use crate::models::{SearchRequest, SuggestionsQuery};
use crate::search::SearchEngine;
use crate::{AppState, Result as SearcherResult, SearcherError};
use axum::body::Body;
use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::Json,
};
use futures_util::StreamExt;
use serde_json::{json, Value};
use sqlx::types::time::OffsetDateTime;
use tracing::{debug, error, info};

pub async fn health_check(State(state): State<AppState>) -> SearcherResult<Json<Value>> {
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
        "service": "searcher",
        "database": "connected",
        "redis": "connected",
        "timestamp": OffsetDateTime::now_utc().to_string()
    })))
}

pub async fn search(
    State(state): State<AppState>,
    Json(request): Json<SearchRequest>,
) -> SearcherResult<Json<Value>> {
    info!("Received search request: {:?}", request);

    let search_engine = SearchEngine::new(
        state.db_pool,
        state.redis_client,
        state.ai_client,
        state.config,
    );

    let response = match search_engine.search(request).await {
        Ok(response) => response,
        Err(e) => {
            error!("Search engine error: {}", e);
            return Err(SearcherError::Internal(e));
        }
    };

    Ok(Json(serde_json::to_value(response)?))
}

pub async fn suggestions(
    State(state): State<AppState>,
    Query(query): Query<SuggestionsQuery>,
) -> SearcherResult<Json<Value>> {
    info!("Received suggestions request: {:?}", query);

    let search_engine = SearchEngine::new(
        state.db_pool,
        state.redis_client,
        state.ai_client,
        state.config,
    );
    let response = search_engine.suggest(&query.q, query.limit()).await?;

    Ok(Json(serde_json::to_value(response)?))
}

pub async fn ai_answer(
    State(state): State<AppState>,
    Json(request): Json<SearchRequest>,
) -> Result<axum::response::Response<Body>, axum::http::StatusCode> {
    info!("Received AI answer request: {:?}", request);

    let search_engine = SearchEngine::new(
        state.db_pool.clone(),
        state.redis_client.clone(),
        state.ai_client.clone(),
        state.config.clone(),
    );

    // Get RAG context by running hybrid search
    let context = match search_engine.get_rag_context(&request).await {
        Ok(context) => context,
        Err(e) => {
            error!("Failed to get RAG context: {}", e);
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    // Build RAG prompt with context and citation instructions
    let prompt = search_engine.build_rag_prompt(&request.query, &context);
    info!("Built RAG prompt of length: {}", prompt.len());
    debug!("RAG prompt: {}", prompt);

    // Stream AI response
    let ai_stream = match state.ai_client.stream_prompt(&prompt).await {
        Ok(stream) => stream,
        Err(e) => {
            error!("Failed to start AI stream: {}", e);
            return Err(StatusCode::BAD_GATEWAY);
        }
    };

    // Convert AI stream to bytes stream
    let byte_stream = ai_stream.map(|chunk| match chunk {
        Ok(text) => Ok(text.into_bytes()),
        Err(e) => {
            error!("AI stream error: {}", e);
            Err(std::io::Error::new(std::io::ErrorKind::Other, e))
        }
    });

    // Create response with streaming body using Body::wrap_stream
    let response = axum::response::Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "text/plain; charset=utf-8")
        .header("Cache-Control", "no-cache")
        .header("Connection", "keep-alive")
        .body(Body::from_stream(byte_stream))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(response)
}
