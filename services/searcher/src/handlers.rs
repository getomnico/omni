use crate::models::{RecentSearchesRequest, SearchRequest, SuggestionsQuery};
use crate::search::SearchEngine;
use crate::{AppState, Result as SearcherResult, SearcherError};
use axum::body::Body;
use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::Json,
};
use futures_util::Stream;
use redis::AsyncCommands;
use serde_json::{json, Value};
use sqlx::types::time::OffsetDateTime;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::sync::Mutex;
use tracing::{debug, error, info};

/// A stream wrapper that collects chunks for caching while forwarding them to the client
struct CachingStream<S> {
    inner: S,
    cache_buffer: Arc<Mutex<String>>,
    cache_key: String,
    redis_client: redis::Client,
}

impl<S> CachingStream<S> {
    fn new(inner: S, cache_key: String, redis_client: redis::Client) -> Self {
        Self {
            inner,
            cache_buffer: Arc::new(Mutex::new(String::new())),
            cache_key,
            redis_client,
        }
    }
}

impl<S> Stream for CachingStream<S>
where
    S: Stream<Item = anyhow::Result<String>> + Unpin,
{
    type Item = Result<Vec<u8>, std::io::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match Pin::new(&mut self.inner).poll_next(cx) {
            Poll::Ready(Some(Ok(chunk))) => {
                // Collect chunk for caching
                let cache_buffer = Arc::clone(&self.cache_buffer);
                let chunk_clone = chunk.clone();
                tokio::spawn(async move {
                    let mut buffer = cache_buffer.lock().await;
                    buffer.push_str(&chunk_clone);
                });

                // Forward chunk to client
                Poll::Ready(Some(Ok(chunk.into_bytes())))
            }
            Poll::Ready(Some(Err(e))) => {
                error!("AI stream error: {}", e);
                Poll::Ready(Some(Err(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    e.to_string(),
                ))))
            }
            Poll::Ready(None) => {
                // Stream ended, cache the complete response
                let cache_buffer = Arc::clone(&self.cache_buffer);
                let cache_key = self.cache_key.clone();
                let redis_client = self.redis_client.clone();

                tokio::spawn(async move {
                    let buffer = cache_buffer.lock().await;
                    if !buffer.is_empty() {
                        if let Ok(mut conn) = redis_client.get_multiplexed_async_connection().await
                        {
                            let _: Result<(), _> =
                                conn.set_ex(&cache_key, buffer.as_str(), 600).await;
                            info!("Cached AI response for key: {}", cache_key);
                        }
                    }
                });

                Poll::Ready(None)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

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

    let response = match search_engine.search(request.clone()).await {
        Ok(response) => response,
        Err(e) => {
            error!("Search engine error: {}", e);
            return Err(SearcherError::Internal(e));
        }
    };

    // Store search history if user_id is provided
    if let Some(user_id) = &request.user_id {
        let is_generated = request.is_generated_query.unwrap_or(false);

        let query_to_store = if is_generated {
            // For AI-generated queries, only cache if original_user_query is provided
            request.original_user_query.as_ref()
        } else {
            // For user queries, cache the query itself
            Some(&request.query)
        };

        if let Some(query) = query_to_store {
            if let Err(e) = search_engine.store_search_history(user_id, query).await {
                // Log the error but don't fail the search request
                error!("Failed to store search history: {}", e);
            }
        }
    }

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

pub async fn recent_searches(
    State(state): State<AppState>,
    Query(query): Query<RecentSearchesRequest>,
) -> SearcherResult<Json<Value>> {
    info!(
        "Received recent searches request for user: {}",
        query.user_id
    );

    let search_engine = SearchEngine::new(
        state.db_pool,
        state.redis_client,
        state.ai_client,
        state.config,
    );

    let response = search_engine.get_recent_searches(&query.user_id).await?;

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

    // Generate cache key for AI answer
    let cache_key = search_engine.generate_ai_cache_key(&request.query);

    // Try to get cached AI response first
    if let Ok(mut conn) = state.redis_client.get_multiplexed_async_connection().await {
        if let Ok(cached_answer) = conn.get::<_, String>(&cache_key).await {
            info!("Cache hit for AI answer query: '{}'", request.query);
            let response = axum::response::Response::builder()
                .status(StatusCode::OK)
                .header("Content-Type", "text/plain; charset=utf-8")
                .header("Cache-Control", "max-age=300") // 5 minutes cache
                .body(Body::from(cached_answer))
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            return Ok(response);
        }
    }

    // Cache miss - generate fresh response
    info!("Cache miss for AI answer query: '{}'", request.query);

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

    // Create caching stream that forwards chunks while collecting for cache
    let caching_stream = CachingStream::new(ai_stream, cache_key, state.redis_client.clone());

    // Create response with streaming body using Body::wrap_stream
    let response = axum::response::Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "text/plain; charset=utf-8")
        .header("Cache-Control", "no-cache")
        .header("Connection", "keep-alive")
        .body(Body::from_stream(caching_stream))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(response)
}
