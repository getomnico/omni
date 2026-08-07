use crate::capabilities_repository::AgentCapabilitiesRepository;
use crate::models::{
    AttributeValuesResponse, CapabilitiesSyncRequest, CapabilitiesSyncResponse,
    CapabilitiesUpsertRequest, CapabilitiesUpsertResponse, CapabilitySearchRequest,
    CapabilitySearchResponse, PeopleSearchResponse, PersonResult, RecentSearchesRequest,
    SearchRequest, SuggestedQuestionsRequest, SuggestedQuestionsResponse, TypeaheadQuery,
    TypeaheadResponse, TypeaheadResult,
};
use crate::search::SearchEngine;
use crate::search_repository::SearchDocumentRepository;
use crate::{AppState, Result as SearcherResult, SearcherError};
use anyhow::anyhow;
use axum::body::Body;
use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::Json,
};
use futures_util::Stream;
use redis::AsyncCommands;
use serde_json::{Value, json};
use shared::{
    ConfigurationRepository, DocumentRepository, GroupRepository, PersonRepository,
    PersonSearchFilter, Repository, UserRepository, models::UserConfiguration,
};
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

async fn hydrate_user_configuration(
    state: &AppState,
    request: &mut SearchRequest,
) -> SearcherResult<()> {
    let Some(user_id) = request
        .user_id
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    else {
        return Ok(());
    };

    let configuration_repo = ConfigurationRepository::new(state.db_pool.pool());
    let configuration_rows = configuration_repo
        .get_user_config(user_id)
        .await
        .map_err(|error| SearcherError::Internal(anyhow!(error)))?;

    request.user_configuration =
        UserConfiguration::from_rows(configuration_rows).map_err(SearcherError::BadRequest)?;

    Ok(())
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

pub fn require_internal_token(headers: &HeaderMap) -> SearcherResult<()> {
    // When the internal service token is configured, every identity-bearing request must
    // present it. This prevents a direct caller from forging another user's identity at
    // the searcher boundary. Tests and local setups that leave the token unset keep the
    // endpoint open for the in-process test harness.
    let Ok(expected) = std::env::var("OMNI_INTERNAL_SERVICE_TOKEN") else {
        return Ok(());
    };
    let provided = headers
        .get("x-omni-internal-token")
        .and_then(|value| value.to_str().ok());
    if provided != Some(expected.as_str()) {
        return Err(SearcherError::BadRequest(
            "Internal service token is required".to_string(),
        ));
    }
    Ok(())
}

pub async fn search(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(mut request): Json<SearchRequest>,
) -> SearcherResult<Json<Value>> {
    info!("Received search request: {:?}", request);

    require_internal_token(&headers)?;

    if matches!(
        request.document_access_scope,
        crate::models::DocumentAccessScope::System
    ) {
        let expected = std::env::var("OMNI_INTERNAL_SERVICE_TOKEN").map_err(|_| {
            SearcherError::BadRequest("System document access is not configured".to_string())
        })?;
        let provided = headers
            .get("x-omni-system-token")
            .and_then(|value| value.to_str().ok());
        if provided != Some(expected.as_str()) {
            return Err(SearcherError::BadRequest(
                "System document access is not authorized".to_string(),
            ));
        }
    }
    hydrate_user_configuration(&state, &mut request).await?;

    let search_engine = SearchEngine::new(
        state.db_pool,
        state.redis_client,
        state.ai_client,
        state.config,
        state.operator_registry,
    )
    .await?;

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
        state.operator_registry,
    )
    .await?;

    let response = search_engine.get_recent_searches(&query.user_id).await?;

    Ok(Json(serde_json::to_value(response)?))
}

pub async fn ai_answer(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(mut request): Json<SearchRequest>,
) -> Result<axum::response::Response<Body>, axum::http::StatusCode> {
    require_internal_token(&headers).map_err(|_| axum::http::StatusCode::BAD_REQUEST)?;
    info!("Received AI answer request: {:?}", request);
    hydrate_user_configuration(&state, &mut request)
        .await
        .map_err(|_| axum::http::StatusCode::BAD_REQUEST)?;

    let search_engine = SearchEngine::new(
        state.db_pool.clone(),
        state.redis_client.clone(),
        state.ai_client.clone(),
        state.config.clone(),
        state.operator_registry.clone(),
    )
    .await
    .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?;

    // Generate cache key for AI answer
    let cache_key = search_engine.generate_ai_cache_key(&request);

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

pub async fn typeahead(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<TypeaheadQuery>,
) -> SearcherResult<Json<Value>> {
    require_internal_token(&headers)?;
    let user_id = query.user_id.clone();
    if user_id.is_empty() {
        info!("typeahead: empty user_id, returning empty");
        return Ok(Json(serde_json::to_value(TypeaheadResponse {
            results: vec![],
            query: query.q,
        })?));
    }

    let user_repo = UserRepository::new(&state.db_pool.pool());
    let user = match user_repo.find_by_id(user_id).await {
        Ok(Some(user)) => user,
        Ok(None) => {
            info!("typeahead: unknown user, returning empty");
            return Ok(Json(serde_json::to_value(TypeaheadResponse {
                results: vec![],
                query: query.q,
            })?));
        }
        Err(e) => {
            error!("typeahead: user lookup failed: {}", e);
            return Err(SearcherError::Internal(anyhow::anyhow!(
                "User lookup failed for typeahead"
            )));
        }
    };

    if !user.is_active {
        info!("typeahead: inactive user, returning empty");
        return Ok(Json(serde_json::to_value(TypeaheadResponse {
            results: vec![],
            query: query.q,
        })?));
    }
    let user_email = user.email;

    // Enforce minimum query length (3 characters after normalization).
    // This prevents broad 1-2 character scans from direct API calls.
    if !crate::typeahead::has_minimum_query_length(&query.q) {
        return Ok(Json(serde_json::to_value(TypeaheadResponse {
            results: vec![],
            query: query.q,
        })?));
    }

    // Resolve current group memberships. Fail closed on DB error.
    let group_repo = GroupRepository::new(&state.db_pool.pool());
    let user_groups = group_repo
        .find_groups_for_user(&user_email)
        .await
        .map_err(|e| {
            error!("typeahead: group lookup failed: {}", e);
            SearcherError::Internal(anyhow::anyhow!("Group lookup failed for typeahead"))
        })?;

    // Fetch the full score-ordered candidate set (no cap).
    let candidates = state.title_index.search_candidates(&query.q).await;

    if candidates.is_empty() {
        return Ok(Json(serde_json::to_value(TypeaheadResponse {
            results: vec![],
            query: query.q,
        })?));
    }

    // Process candidates in bounded batches until the requested limit is
    // filled or all candidates are exhausted.
    let doc_repo = DocumentRepository::new(state.db_pool.pool());
    let batch_size = 100;
    let mut accessible: Vec<TypeaheadResult> = Vec::new();
    let limit = query.limit();

    for chunk in candidates.chunks(batch_size) {
        if accessible.len() >= limit {
            break;
        }

        let ids: Vec<String> = chunk.iter().map(|c| c.document_id.clone()).collect();
        let allowed = doc_repo
            .filter_accessible_titles(&ids, &user_email, &user_groups)
            .await
            .map_err(|e| {
                error!("typeahead: ACL filter failed: {}", e);
                SearcherError::Internal(anyhow::anyhow!("Permission check failed for typeahead"))
            })?;

        // Collect accessible results in ranked order.
        for c in chunk {
            if accessible.len() >= limit {
                break;
            }
            if !allowed.contains(&c.document_id) {
                continue;
            }
            accessible.push(TypeaheadResult {
                document_id: c.document_id.clone(),
                title: c.title.clone(),
                url: c.url.clone(),
                source_id: c.source_id.clone(),
                source_type: c.source_type.clone(),
                content_type: c.content_type.clone(),
            });
        }
    }

    let response = TypeaheadResponse {
        results: accessible,
        query: query.q,
    };
    Ok(Json(serde_json::to_value(response)?))
}

// TODO: Make this a GET request, this should not be POST
pub async fn suggested_questions(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<SuggestedQuestionsRequest>,
) -> SearcherResult<Json<SuggestedQuestionsResponse>> {
    require_internal_token(&headers)?;
    info!("Received suggested questions request");

    let user_repo = UserRepository::new(&state.db_pool.pool());
    let user = match user_repo.find_by_id(request.user_id.clone()).await {
        Ok(Some(user)) if user.is_active => user,
        Ok(Some(_)) | Ok(None) => {
            error!("Active user not found for user_id {}", request.user_id);
            return Err(SearcherError::NotFound(format!(
                "User not found for user_id {}",
                request.user_id
            )));
        }
        Err(e) => {
            error!(
                "Failed to fetch user for user_id {}: {:?}",
                request.user_id, e
            );
            return Err(anyhow!(
                "Failed to fetch user for user_id {}: {:?}",
                request.user_id,
                e
            )
            .into());
        }
    };

    let response = state
        .suggested_questions_generator
        .get_suggested_questions(&user.email)
        .await?;

    Ok(Json(response))
}

#[derive(Debug, serde::Deserialize)]
pub struct AttributeValuesQuery {
    pub keys: String,
    pub limit: Option<i64>,
    pub user_id: String,
    pub document_access_scope: Option<crate::models::DocumentAccessScope>,
}

#[derive(Debug, serde::Deserialize)]
pub struct PeopleSearchQuery {
    pub q: String,
    pub limit: Option<i64>,
    pub department: Option<String>,
    pub office_location: Option<String>,
    pub work_country: Option<String>,
    pub employee_type: Option<String>,
}

pub async fn people_search(
    State(state): State<AppState>,
    Query(query): Query<PeopleSearchQuery>,
) -> SearcherResult<Json<PeopleSearchResponse>> {
    let person_repo = PersonRepository::new(state.db_pool.pool());
    let limit = query.limit.unwrap_or(10).min(50);

    let filter = PersonSearchFilter {
        department: non_empty(&query.department),
        office_location: non_empty(&query.office_location),
        work_country: non_empty(&query.work_country),
        employee_type: non_empty(&query.employee_type),
    };

    let results = person_repo
        .search_people(&query.q, limit, &filter)
        .await
        .map_err(|e| SearcherError::Internal(anyhow!("People search failed: {}", e)))?;

    let people = results
        .into_iter()
        .map(|p| PersonResult {
            id: p.id,
            email: p.email,
            display_name: p.display_name,
            given_name: p.given_name,
            middle_name: p.middle_name,
            surname: p.surname,
            job_title: p.job_title,
            department: p.department,
            division: p.division,
            company_name: p.company_name,
            office_location: p.office_location,
            work_country: p.work_country,
            employee_id: p.employee_id,
            employee_type: p.employee_type,
            cost_center: p.cost_center,
            grade: p.grade,
            band: p.band,
            confirmation_status: p.confirmation_status,
            employment_start_date: p.employment_start_date,
            employment_end_date: p.employment_end_date,
            score: p.score,
        })
        .collect();

    Ok(Json(PeopleSearchResponse { people }))
}

fn non_empty(value: &Option<String>) -> Option<String> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
}

pub async fn attribute_values(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<AttributeValuesQuery>,
) -> SearcherResult<Json<AttributeValuesResponse>> {
    require_internal_token(&headers)?;
    let keys: Vec<String> = query
        .keys
        .split(',')
        .map(|k| k.trim().to_string())
        .filter(|k| !k.is_empty())
        .collect();

    if keys.is_empty() {
        return Err(SearcherError::BadRequest(
            "keys parameter is required".to_string(),
        ));
    }

    let limit = query.limit.unwrap_or(25).min(100);
    let scope = query.document_access_scope.unwrap_or_default();
    let user_email = match scope {
        crate::models::DocumentAccessScope::Public => {
            shared::db::repositories::document::PUBLIC_ONLY_PERMISSION_IDENTITY.to_string()
        }
        crate::models::DocumentAccessScope::User => {
            UserRepository::new(state.db_pool.pool())
                .find_by_id(query.user_id)
                .await
                .map_err(|e| SearcherError::Internal(anyhow!(e)))?
                .filter(|user| user.is_active)
                .ok_or_else(|| {
                    SearcherError::BadRequest("Unknown or inactive user identity".to_string())
                })?
                .email
        }
        crate::models::DocumentAccessScope::System => {
            return Err(SearcherError::BadRequest(
                "System scope is not available on this endpoint".to_string(),
            ));
        }
    };
    let repo = SearchDocumentRepository::new(
        &state.db_pool,
        Some(&user_email),
        matches!(scope, crate::models::DocumentAccessScope::Public),
    );
    let attributes = repo
        .get_distinct_attribute_values(&keys, limit)
        .await
        .map_err(|e| SearcherError::Internal(anyhow!("Failed to fetch attribute values: {}", e)))?;

    Ok(Json(AttributeValuesResponse { attributes }))
}

fn validate_capabilities(capabilities: &[crate::models::CapabilityUpsert]) -> SearcherResult<()> {
    if capabilities.len() > 500 {
        return Err(SearcherError::BadRequest(
            "capabilities batch is limited to 500 items".to_string(),
        ));
    }
    if capabilities.iter().any(|capability| {
        capability.id.trim().is_empty()
            || capability.capability_type.trim().is_empty()
            || capability.name.trim().is_empty()
            || capability.search_text.trim().is_empty()
    }) {
        return Err(SearcherError::BadRequest(
            "capability id, capability_type, name, and search_text are required".to_string(),
        ));
    }
    Ok(())
}

pub async fn capabilities_upsert(
    State(state): State<AppState>,
    Json(request): Json<CapabilitiesUpsertRequest>,
) -> SearcherResult<Json<CapabilitiesUpsertResponse>> {
    validate_capabilities(&request.capabilities)?;

    let repo = AgentCapabilitiesRepository::new(state.db_pool.pool());
    repo.upsert_many(&request.capabilities)
        .await
        .map_err(|e| SearcherError::Internal(anyhow!("Capability upsert failed: {}", e)))?;

    Ok(Json(CapabilitiesUpsertResponse {
        upserted: request.capabilities.len(),
    }))
}

pub async fn capabilities_sync(
    State(state): State<AppState>,
    Json(request): Json<CapabilitiesSyncRequest>,
) -> SearcherResult<Json<CapabilitiesSyncResponse>> {
    if request.publisher_id.trim().is_empty() || request.capability_type.trim().is_empty() {
        return Err(SearcherError::BadRequest(
            "publisher_id and capability_type are required".to_string(),
        ));
    }
    validate_capabilities(&request.capabilities)?;
    if request
        .capabilities
        .iter()
        .any(|capability| capability.capability_type != request.capability_type)
    {
        return Err(SearcherError::BadRequest(
            "all capabilities must match request capability_type".to_string(),
        ));
    }

    let repo = AgentCapabilitiesRepository::new(state.db_pool.pool());
    let deleted = repo
        .sync_publisher(
            &request.publisher_id,
            &request.capability_type,
            &request.capabilities,
        )
        .await
        .map_err(|e| SearcherError::Internal(anyhow!("Capability sync failed: {}", e)))?;

    Ok(Json(CapabilitiesSyncResponse {
        upserted: request.capabilities.len(),
        deleted,
    }))
}

pub async fn capabilities_search(
    State(state): State<AppState>,
    Json(request): Json<CapabilitySearchRequest>,
) -> SearcherResult<Json<CapabilitySearchResponse>> {
    if request.query.trim().is_empty() {
        return Err(SearcherError::BadRequest(
            "query cannot be empty".to_string(),
        ));
    }
    if request.capability_type.trim().is_empty() {
        return Err(SearcherError::BadRequest(
            "capability_type is required".to_string(),
        ));
    }

    let repo = AgentCapabilitiesRepository::new(state.db_pool.pool());
    let results = repo
        .search(
            &request.capability_type,
            &request.query,
            request.limit(),
            request.allowed_ids.as_deref(),
            request.allowed_source_ids.as_deref(),
        )
        .await
        .map_err(|e| SearcherError::Internal(anyhow!("Capability search failed: {}", e)))?;

    Ok(Json(CapabilitySearchResponse { results }))
}
