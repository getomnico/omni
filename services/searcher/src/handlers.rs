use crate::models::{SearchRequest, SuggestionsQuery};
use crate::search::SearchEngine;
use crate::{AppState, Result as SearcherResult};
use axum::{
    extract::{Query, State},
    response::Json,
};
use serde_json::{json, Value};
use sqlx::types::time::OffsetDateTime;
use tracing::info;

pub async fn health_check(State(state): State<AppState>) -> SearcherResult<Json<Value>> {
    sqlx::query("SELECT 1").execute(&state.db_pool).await?;

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

    let search_engine = SearchEngine::new(state.db_pool);
    let response = search_engine.search(request).await?;

    Ok(Json(serde_json::to_value(response)?))
}

pub async fn suggestions(
    State(state): State<AppState>,
    Query(query): Query<SuggestionsQuery>,
) -> SearcherResult<Json<Value>> {
    info!("Received suggestions request: {:?}", query);

    let search_engine = SearchEngine::new(state.db_pool);
    let response = search_engine.suggest(&query.q, query.limit()).await?;

    Ok(Json(serde_json::to_value(response)?))
}
