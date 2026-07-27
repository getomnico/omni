use crate::models::{SuggestedQuestion, SuggestedQuestionsResponse};
use crate::{Result as SearcherResult, SearcherError};
use anyhow::{anyhow, Context, Result};
use dashmap::DashSet;
use futures_util::StreamExt;
use redis::AsyncCommands;
use redis::Client as RedisClient;
use shared::utils::safe_str_slice;
use shared::{
    AIClient, DatabasePool, DocumentRepository, GroupRepository, ObjectStorage, SourceType,
};
use std::collections::HashSet;
use std::sync::Arc;
use tracing::{debug, error, info, warn};

// Bump the version suffix whenever the generation prompt or validation changes
// so previously cached (and now undesirable) suggestions are invalidated.
const REDIS_CACHE_KEY: &str = "suggested_questions:v3";
const CACHE_TTL_SECONDS: u64 = 86400; // 24 hours
const MAX_RETRIES: usize = 5;
/// Source types whose documents are predominantly code or technical material and
/// therefore make poor "Try asking" suggestions (e.g. a GitHub repo yields queries
/// like "Git repository object storage documentation"). They are excluded from the
/// random document selection; per-document SKIP handling in the generator still
/// filters technical content out of the remaining sources.
const SUGGESTION_EXCLUDED_SOURCE_TYPES: &[SourceType] = &[SourceType::Github];
const QUESTION_PROMPT_TEMPLATE: &str = r#"You are helping generate example search queries for a workplace search tool that indexes a company's documents, emails, and files. The result is shown as a clickable "Try asking" suggestion on the home screen, so it must read like something a real employee would naturally ask.

You are given an excerpt from ONE indexed document. Based on the topic it covers, write ONE natural question that a colleague might ask to find this kind of information.

Rules:
- Write a full, natural question, phrased the way a person actually speaks, and end it with a question mark. Examples: "How do we handle customer refunds?", "What are the onboarding steps for new engineers?", "Who is responsible for Q3 budget planning?"
- Do NOT output keyword phrases, document titles, or noun phrases such as "Git repository object storage documentation". It must be a real question.
- Ask from the perspective of someone who has NOT seen this document and is looking for the knowledge it contains.
- Ignore document metadata and structure (URLs, file paths, IDs, timestamps, formatting, raw code, configuration values).

If this document is not a good basis for a useful business question — for example it is source code, configuration, logs, build or CI output, auto-generated content, technical/system documentation, boilerplate, or has no clear business topic an employee would search for — then respond with exactly the single word SKIP and nothing else.

Output only the question itself (or SKIP), with no quotes and no prefix like "Question:" or "Query:".

Document excerpt:
{content}"#;

const TASK_PROMPT_TEMPLATE: &str = r#"You are helping generate example task prompts for a workplace AI tool that can answer questions and also perform tasks like running analyses, summarizing content, or creating charts. The result is shown as a clickable suggestion on the home screen.

You are given an excerpt from ONE indexed document. Based on what it covers, write ONE natural task instruction that a colleague might give the AI — something actionable it could do with this kind of content.

Rules:
- Write a concise imperative instruction in the style of "Summarize the key takeaways from the Q3 board meeting", "Show a chart of sales by region from the Q4 report", or "List the action items from the last sprint retro".
- Do NOT write a question — the output must be an instruction, not an inquiry.
- Do NOT output keyword phrases, document titles, or noun phrases.
- Phrase it from the perspective of someone who wants the AI to do something useful with this content.
- Ignore document metadata and structure (URLs, file paths, IDs, timestamps, formatting, raw code, configuration values).

If this document is not a good basis for a useful task instruction — for example it is source code, configuration, logs, build or CI output, auto-generated content, technical/system documentation, boilerplate, or has no clear business action an employee would want performed — then respond with exactly the single word SKIP and nothing else.

Output only the task instruction itself (or SKIP), with no quotes and no prefix like "Task:" or "Instruction:".

Document excerpt:
{content}"#;

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
        info!(
            "Cache miss for suggested questions, checking for in-flight generations for this user before proceeding."
        );
        if self.in_flight.contains(user_email) {
            info!("Suggested questions generation already in progress");
            return Ok(SuggestedQuestionsResponse { questions: vec![] });
        }

        info!("No in-flight generation found, starting new generation task");
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
                            info!(
                                "Successfully generated and cached {} suggested questions",
                                count
                            );
                            // Remove the user from the in-flight map to allow future requests to go through
                            in_flight.remove(&user_email);
                        }
                        Err(_) => {
                            error!("Failed to generate suggested questions");
                            // Remove the user from the in-flight map to allow future requests to go through
                            in_flight.remove(&user_email);
                        }
                    }
                } else {
                    info!("Another generation task started while we were waiting");
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
        // Random fetches across retry attempts can re-draw the same document, and
        // distinct documents can yield identical questions. Track both so the
        // suggestions we return stay unique.
        let mut seen_doc_ids: HashSet<String> = HashSet::new();
        let mut seen_questions: HashSet<String> = HashSet::new();
        let mut attempts = 0;

        let num_questions = 9;
        info!(
            "Beginning suggestion generation loop (target: {} suggestions, max attempts: {})",
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

            let group_repo = GroupRepository::new(db_pool.pool());
            let user_groups: Vec<String> = group_repo
                .find_groups_for_user(user_email)
                .await
                .unwrap_or_default();
            // Exclude documents already consumed in earlier attempts at the query
            // level, so every fetched document is new and attempts don't spin on
            // re-drawn rows (the seen_doc_ids guard below stays as a safety net).
            let already_seen: Vec<String> = seen_doc_ids.iter().cloned().collect();
            match doc_repo
                .fetch_random_documents(
                    user_email,
                    &user_groups,
                    needed,
                    SUGGESTION_EXCLUDED_SOURCE_TYPES,
                    &already_seen,
                )
                .await
            {
                Ok(docs) => {
                    let num_docs_fetched = docs.len();
                    info!(
                        "Fetched {} random document(s) for suggestion generation",
                        num_docs_fetched
                    );

                    let content_ids: Vec<String> =
                        docs.iter().filter_map(|d| d.content_id.clone()).collect();

                    debug!("Fetching content for generating suggested questions");
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
                        // Skip documents already handled in an earlier attempt so we
                        // neither spend an AI call nor surface a duplicate suggestion.
                        if !seen_doc_ids.insert(doc.id.clone()) {
                            continue;
                        }

                        debug!(
                            "Processing document (content length: {} chars)",
                            content.len()
                        );

                        // Every third suggestion is a task instruction; the rest are questions.
                        let use_task_prompt = questions.len() % 3 == 2;
                        let result = if use_task_prompt {
                            Self::generate_task_from_document(&ai_client, &doc.id, &content).await
                        } else {
                            Self::generate_question_from_document(&ai_client, &doc.id, &content)
                                .await
                        };
                        match result {
                            Ok(question) => {
                                // Two different documents can produce the same generic
                                // suggestion; keep the displayed suggestions distinct.
                                if !seen_questions.insert(question.to_lowercase()) {
                                    debug!("Skipping duplicate suggestion text");
                                    continue;
                                }
                                questions.push(SuggestedQuestion {
                                    question: question.clone(),
                                    document_id: doc.id.clone(),
                                });
                                info!("Generated suggestion {}/{}", questions.len(), num_questions);

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
                                debug!("Caching questions in Redis (TTL: {}s)", CACHE_TTL_SECONDS);
                                redis_conn
                                    .set_ex::<_, _, ()>(cache_key, &json_str, CACHE_TTL_SECONDS)
                                    .await
                                    .context("Failed to cache questions in Redis")?;

                                info!(
                                    "Successfully cached {} suggested suggestion(s) in Redis (TTL: {} hours)",
                                    response.questions.len(),
                                    CACHE_TTL_SECONDS / 3600
                                );
                            }
                            Err(_) => {
                                warn!("Failed to generate suggestion");
                            }
                        }

                        if questions.len() >= num_questions {
                            info!(
                                "Target of {} suggestions reached, stopping generation",
                                num_questions
                            );
                            break;
                        }
                    }

                    if num_docs_fetched < needed {
                        debug!(
                            "User has only {} documents, skipping further attempts",
                            num_docs_fetched
                        );
                        break;
                    }
                }
                Err(_) => {
                    error!("Failed to fetch random documents on attempt {}", attempts);
                }
            }
        }

        if questions.is_empty() {
            error!(
                "Failed to generate any suggestions after {} attempts",
                attempts
            );
            return Err(anyhow!(
                "Failed to generate any suggestions after {} attempts",
                attempts
            ));
        }

        info!(
            "Suggestion generation complete: {} suggestion(s) generated after {} attempt(s)",
            questions.len(),
            attempts
        );

        Ok(questions.len())
    }

    async fn generate_suggestion_from_document(
        ai_client: &AIClient,
        document_id: &str,
        content: &str,
        prompt_template: &str,
        expect_question_mark: bool,
    ) -> Result<String> {
        let excerpt = if content.len() > 2000 {
            debug!("Truncating content from {} to 2000 chars", content.len());
            safe_str_slice(content, 0, 2000)
        } else {
            content
        };

        let prompt = prompt_template.replace("{content}", excerpt);

        let mut stream = ai_client
            .stream_prompt(&prompt)
            .await
            .context("Failed to start AI stream")?;

        let mut output = String::new();
        while let Some(chunk_result) = stream.next().await {
            match chunk_result {
                Ok(chunk) => output.push_str(&chunk),
                Err(e) => return Err(anyhow!("Error in AI stream: {}", e)),
            }
        }

        let output = output.trim().trim_matches('"').trim().to_string();

        if output.is_empty() {
            return Err(anyhow!("AI service returned empty output"));
        }

        let normalized =
            output.trim_end_matches(|c: char| c == '.' || c == '!' || c.is_whitespace());
        if normalized.eq_ignore_ascii_case("skip") {
            info!("Document deemed unsuitable for suggestion (model returned SKIP)");
            return Err(anyhow!(
                "Document unsuitable for suggestion generation (SKIP)"
            ));
        }

        if expect_question_mark && !output.contains('?') {
            warn!("Discarding non-question suggestion");
            return Err(anyhow!("Generated suggestion was not a question"));
        }

        info!("Successfully generated suggestion");

        Ok(output)
    }

    async fn generate_question_from_document(
        ai_client: &AIClient,
        document_id: &str,
        content: &str,
    ) -> Result<String> {
        Self::generate_suggestion_from_document(
            ai_client,
            document_id,
            content,
            QUESTION_PROMPT_TEMPLATE,
            true,
        )
        .await
    }

    async fn generate_task_from_document(
        ai_client: &AIClient,
        document_id: &str,
        content: &str,
    ) -> Result<String> {
        Self::generate_suggestion_from_document(
            ai_client,
            document_id,
            content,
            TASK_PROMPT_TEMPLATE,
            false,
        )
        .await
    }
}
