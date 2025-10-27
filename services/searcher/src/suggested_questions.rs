use crate::models::{SuggestedQuestion, SuggestedQuestionsResponse};
use crate::{Result as SearcherResult, SearcherError};
use anyhow::{anyhow, Context, Result};
use dashmap::DashSet;
use futures_util::StreamExt;
use redis::AsyncCommands;
use redis::Client as RedisClient;
use shared::utils::safe_str_slice;
use shared::{AIClient, DatabasePool, DocumentRepository, ObjectStorage};
use std::sync::Arc;
use tracing::{debug, error, info, warn};

const REDIS_CACHE_KEY: &str = "suggested_questions:v1";
const CACHE_TTL_SECONDS: u64 = 86400; // 7 days
const MAX_RETRIES: usize = 5;
const QUESTION_PROMPT_TEMPLATE: &str = r#"Given the following document excerpt, generate ONE specific question that someone might ask to find this document. The question should be natural, specific, and answerable by the document content.

Document excerpt:
{content}

Generate only the question, nothing else. Do not include quotes or prefixes like "Question:"."#;

pub struct SuggestedQuestionsGenerator {
    redis_client: RedisClient,
    db_pool: DatabasePool,
    content_storage: Arc<dyn ObjectStorage>,
    ai_client: AIClient,
    in_flight: Arc<DashSet<String>>,
}

impl SuggestedQuestionsGenerator {
    pub fn new(
        redis_client: RedisClient,
        db_pool: DatabasePool,
        content_storage: Arc<dyn ObjectStorage>,
        ai_client: AIClient,
    ) -> Self {
        Self {
            redis_client,
            db_pool,
            content_storage,
            ai_client,
            in_flight: Arc::new(DashSet::new()),
        }
    }

    pub async fn get_suggested_questions(
        &self,
        user_email: &str,
    ) -> SearcherResult<SuggestedQuestionsResponse> {
        let mut redis = self.redis_client.get_multiplexed_async_connection().await?;

        if let Ok(cached) = redis
            .get::<_, String>(format!("{}:{}", REDIS_CACHE_KEY, user_email))
            .await
        {
            info!("Cache hit for suggested questions");
            let response: SuggestedQuestionsResponse =
                serde_json::from_str(&cached).map_err(|e| SearcherError::Serialization(e))?;
            return Ok(response);
        }

        // Check for existing in-flight suggested question generation tasks
        info!("Cache miss for suggested questions, checking for in-flight generations for this user before proceeding.");
        if self.in_flight.contains(user_email) {
            info!(
                "Suggested questions generation already in progress for user {}",
                user_email
            );
            return Ok(SuggestedQuestionsResponse { questions: vec![] });
        }

        info!(
            "No in-flight generation found, starting new generation task for user {}",
            user_email
        );
        tokio::spawn({
            let user_email = user_email.to_string();
            let in_flight = Arc::clone(&self.in_flight);
            let db_pool = self.db_pool.clone();
            let redis_client = self.redis_client.clone();
            let content_storage = self.content_storage.clone();
            let ai_client = self.ai_client.clone();
            async move {
                if in_flight.insert(user_email.clone()) {
                    match Self::generate_and_cache_questions(
                        db_pool,
                        redis_client,
                        content_storage,
                        ai_client,
                        &user_email,
                    )
                    .await
                    {
                        Ok(count) => {
                            info!("Successfully generated and cached {} suggested questions for user {}", count, user_email);
                            // Remove the user from the in-flight map to allow future requests to go through
                            in_flight.remove(&user_email);
                        }
                        Err(e) => {
                            error!(
                                "Failed to generate suggested questions for user {}: {:?}",
                                user_email, e
                            );
                            // Remove the user from the in-flight map to allow future requests to go through
                            in_flight.remove(&user_email);
                        }
                    }
                } else {
                    info!(
                        "Another generation task started for user {} while we were waiting",
                        user_email
                    );
                }
            }
        });

        Ok(SuggestedQuestionsResponse { questions: vec![] })
    }

    async fn generate_and_cache_questions(
        db_pool: DatabasePool,
        redis_client: RedisClient,
        content_storage: Arc<dyn ObjectStorage>,
        ai_client: AIClient,
        user_email: &str,
    ) -> Result<usize> {
        let mut questions = Vec::new();
        let mut attempts = 0;

        let num_questions = 9;
        info!(
            "Beginning question generation loop (target: {} questions, max attempts: {})",
            num_questions, MAX_RETRIES
        );

        let doc_repo = DocumentRepository::new(&db_pool.pool());
        while questions.len() < num_questions && attempts < MAX_RETRIES {
            attempts += 1;
            let needed = num_questions - questions.len();
            debug!(
                "Attempt {}/{}: Need {} more question(s)",
                attempts, MAX_RETRIES, needed
            );

            match doc_repo.fetch_random_documents(user_email, needed).await {
                Ok(docs) => {
                    let num_docs_fetched = docs.len();
                    info!(
                        "Fetched {} random document(s) for question generation",
                        num_docs_fetched
                    );

                    let content_ids: Vec<String> =
                        docs.iter().filter_map(|d| d.content_id.clone()).collect();

                    debug!(
                        "Fetching content IDs {:?} for generating suggested questions",
                        content_ids
                    );
                    let content_map = content_storage.batch_get_text(content_ids).await?;

                    // Build contents vector in the same order as documents
                    let contents: Vec<String> = docs
                        .iter()
                        .map(|doc| {
                            let x = doc
                                .content_id
                                .as_ref()
                                .and_then(|cid| content_map.get(cid).cloned())
                                .with_context(|| {
                                    format!("Failed to get content for document {}", doc.id)
                                });
                            x
                        })
                        .collect::<Result<Vec<_>>>()?;

                    for (doc, content) in docs.into_iter().zip(contents) {
                        debug!(
                            "Processing document {} [id={}] (content length: {} chars)",
                            doc.title,
                            doc.id,
                            content.len()
                        );

                        match Self::generate_question_from_document(&ai_client, &doc.id, &content)
                            .await
                        {
                            Ok(question) => {
                                questions.push(SuggestedQuestion {
                                    question: question.clone(),
                                    document_id: doc.id.clone(),
                                });
                                info!(
                                    "Generated question {}/{}: \"{}\" (from document: {})",
                                    questions.len(),
                                    num_questions,
                                    question,
                                    doc.id
                                );

                                // Cache the questions
                                debug!("Serializing {} question(s) to JSON", questions.len());
                                let response = SuggestedQuestionsResponse {
                                    questions: questions.clone(),
                                };
                                let json_str = serde_json::to_string(&response)
                                    .context("Failed to serialize questions to JSON")?;

                                debug!("Connecting to Redis to cache questions");
                                let mut redis_conn = redis_client
                                    .get_multiplexed_async_connection()
                                    .await
                                    .context("Failed to connect to Redis")?;

                                let cache_key = format!("{}:{}", REDIS_CACHE_KEY, user_email);
                                debug!(
                                    "Caching questions in Redis with key: {}, TTL: {}s",
                                    cache_key, CACHE_TTL_SECONDS
                                );
                                redis_conn
                                    .set_ex::<_, _, ()>(cache_key, &json_str, CACHE_TTL_SECONDS)
                                    .await
                                    .context("Failed to cache questions in Redis")?;

                                info!(
                                    "Successfully cached {} suggested question(s) in Redis (TTL: {} hours)",
                                    response.questions.len(),
                                    CACHE_TTL_SECONDS / 3600
                                );
                            }
                            Err(e) => {
                                warn!("Failed to generate question for document {}: {}", doc.id, e);
                            }
                        }

                        if questions.len() >= num_questions {
                            info!(
                                "Target of {} questions reached, stopping generation",
                                num_questions
                            );
                            break;
                        }
                    }

                    if num_docs_fetched < needed {
                        debug!(
                            "User {} has only {} documents, skipping further attempts",
                            user_email, num_docs_fetched
                        );
                        break;
                    }
                }
                Err(e) => {
                    error!(
                        "Failed to fetch random documents on attempt {}: {}",
                        attempts, e
                    );
                }
            }
        }

        if questions.is_empty() {
            error!(
                "Failed to generate any questions after {} attempts",
                attempts
            );
            return Err(anyhow!(
                "Failed to generate any questions after {} attempts",
                attempts
            ));
        }

        info!(
            "Question generation complete: {} question(s) generated after {} attempt(s)",
            questions.len(),
            attempts
        );

        Ok(questions.len())
    }

    async fn generate_question_from_document(
        ai_client: &AIClient,
        document_id: &str,
        content: &str,
    ) -> Result<String> {
        // Take first 2000 characters of content
        let excerpt = if content.len() > 2000 {
            debug!(
                "Truncating content from {} to 2000 chars for document {}",
                content.len(),
                document_id
            );
            safe_str_slice(content, 0, 2000)
        } else {
            content
        };

        let prompt = QUESTION_PROMPT_TEMPLATE.replace("{content}", excerpt);
        debug!(
            "Generated prompt for document {} (length: {} chars)",
            document_id,
            prompt.len()
        );

        info!(
            "Calling AI service to generate question for document {}",
            document_id
        );

        // Call AI service using stream_prompt and collect the full response
        let mut stream = ai_client
            .stream_prompt(&prompt)
            .await
            .context("Failed to start AI stream")?;

        debug!("AI stream started, collecting response chunks");
        let mut question = String::new();
        let mut chunk_count = 0;

        while let Some(chunk_result) = stream.next().await {
            match chunk_result {
                Ok(chunk) => {
                    chunk_count += 1;
                    debug!("Received chunk {} ({} bytes)", chunk_count, chunk.len());
                    question.push_str(&chunk);
                }
                Err(e) => {
                    error!("Error in AI stream for document {}: {}", document_id, e);
                    return Err(anyhow!("Error in AI stream: {}", e));
                }
            }
        }

        debug!(
            "AI stream complete for document {}: received {} chunks, total {} chars",
            document_id,
            chunk_count,
            question.len()
        );

        let question = question.trim().to_string();

        if question.is_empty() {
            warn!(
                "AI service returned empty question for document {}",
                document_id
            );
            return Err(anyhow!("AI service returned empty question"));
        }

        info!(
            "Successfully generated question for document {}: \"{}\"",
            document_id, question
        );

        Ok(question)
    }
}
