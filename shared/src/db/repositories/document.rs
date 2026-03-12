use crate::{
    db::error::DatabaseError,
    models::{AttributeFilter, DateFilter, Document, Facet, FacetValue},
    SourceType,
};
use serde_json::Value as JsonValue;
use sqlx::{FromRow, PgPool};
use std::collections::{HashMap, HashSet};
use time::{self, OffsetDateTime};
use tracing::debug;

#[derive(FromRow)]
pub struct SearchHit {
    #[sqlx(flatten)]
    pub document: Document,
    pub score: f32,
    #[sqlx(default)]
    pub content_snippets: Option<Vec<String>>,
}

#[derive(FromRow)]
pub struct TitleEntry {
    pub id: String,
    pub title: String,
    pub url: Option<String>,
    pub source_id: String,
}

pub struct DocumentRepository {
    pool: PgPool,
}

impl DocumentRepository {
    pub fn new(pool: &PgPool) -> Self {
        Self { pool: pool.clone() }
    }

    /// Generate SQL condition to check if user has permission to access document
    fn generate_permission_filter(&self, user_email: &str) -> String {
        format!(
            r#"(
                permissions @@@ 'public:true' OR
                permissions @@@ 'users:{}' OR
                permissions @@@ 'groups:{}'
            )"#,
            user_email, user_email
        )
    }

    pub async fn find_by_id(&self, id: &str) -> Result<Option<Document>, DatabaseError> {
        let document = sqlx::query_as::<_, Document>(
            r#"
            SELECT id, source_id, external_id, title, content_id, content_type,
                   file_size, file_extension, url,
                   metadata, permissions, attributes, created_at, updated_at, last_indexed_at
            FROM documents
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(document)
    }

    pub async fn find_by_ids(&self, ids: &[String]) -> Result<Vec<Document>, DatabaseError> {
        if ids.is_empty() {
            return Ok(vec![]);
        }

        let documents = sqlx::query_as::<_, Document>(
            r#"
            SELECT id, source_id, external_id, title, content_id, content_type,
                   file_size, file_extension, url,
                   metadata, permissions, attributes, created_at, updated_at, last_indexed_at
            FROM documents
            WHERE id = ANY($1)
            "#,
        )
        .bind(ids)
        .fetch_all(&self.pool)
        .await?;

        Ok(documents)
    }

    pub async fn find_all(&self, limit: i64, offset: i64) -> Result<Vec<Document>, DatabaseError> {
        let documents = sqlx::query_as::<_, Document>(
            r#"
            SELECT id, source_id, external_id, title, content_id, content_type,
                   file_size, file_extension, url,
                   metadata, permissions, attributes, created_at, updated_at, last_indexed_at
            FROM documents
            ORDER BY created_at DESC
            LIMIT $1 OFFSET $2
            "#,
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await?;

        Ok(documents)
    }

    pub async fn list_all_ids(&self) -> Result<Vec<String>, DatabaseError> {
        let rows = sqlx::query_scalar::<_, String>("SELECT id FROM documents")
            .fetch_all(&self.pool)
            .await?;
        Ok(rows)
    }

    pub async fn fetch_all_title_entries(&self) -> Result<Vec<TitleEntry>, DatabaseError> {
        let entries = sqlx::query_as::<_, TitleEntry>(
            r#"
            SELECT d.id, d.title, d.url, d.source_id
            FROM documents d
            JOIN sources s ON d.source_id = s.id
            WHERE NOT s.is_deleted
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(entries)
    }

    pub async fn fetch_random_documents(
        &self,
        user_email: &str,
        count: usize,
    ) -> Result<Vec<Document>, DatabaseError> {
        let permission_filter = &self.generate_permission_filter(user_email);

        let query = format!(
            r#"
            SELECT *
            FROM documents d
            WHERE d.content_id IS NOT NULL
                AND {}
            ORDER BY RANDOM()
            LIMIT $1
        "#,
            permission_filter
        );

        let documents = sqlx::query_as::<_, Document>(&query)
            .bind(count as i32)
            .fetch_all(&self.pool)
            .await?;

        Ok(documents)
    }

    pub async fn fetch_active_source_ids(
        &self,
        source_types: Option<&[SourceType]>,
    ) -> Result<Vec<String>, DatabaseError> {
        let source_ids: Vec<String> = if let Some(source_types) = source_types {
            sqlx::query_scalar(
                r#"SELECT id FROM sources WHERE source_type = ANY($1) AND NOT is_deleted"#,
            )
            .bind(source_types)
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query_scalar(r#"SELECT id FROM sources WHERE NOT is_deleted"#)
                .fetch_all(&self.pool)
                .await?
        };

        Ok(source_ids)
    }

    pub async fn fetch_all_permission_users(&self) -> Result<Vec<String>, DatabaseError> {
        let users: Vec<String> = sqlx::query_scalar(
            r#"SELECT DISTINCT lower(elem)
               FROM documents, jsonb_array_elements_text(permissions->'users') AS elem
               WHERE permissions->'users' IS NOT NULL"#,
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(users)
    }

    pub async fn fetch_max_last_indexed_at(&self) -> Result<Option<OffsetDateTime>, DatabaseError> {
        let max_ts: Option<OffsetDateTime> =
            sqlx::query_scalar(r#"SELECT MAX(last_indexed_at) FROM documents"#)
                .fetch_one(&self.pool)
                .await?;

        Ok(max_ts)
    }

    fn build_common_filters(
        &self,
        filters: &mut Vec<String>,
        param_idx: &mut usize,
        source_ids: &[String],
        content_types: Option<&[String]>,
        attribute_filters: Option<&HashMap<String, AttributeFilter>>,
        user_email: Option<&str>,
        date_filter: Option<&DateFilter>,
    ) {
        if !source_ids.is_empty() {
            filters.push(format!("source_id = ANY(${})", param_idx));
            *param_idx += 1;
        }

        let has_content_types = content_types.is_some_and(|ct| !ct.is_empty());
        if has_content_types {
            filters.push(format!("content_type = ANY(${})", param_idx));
            *param_idx += 1;
        }

        if let Some(attr_filters) = attribute_filters {
            for (key, filter) in attr_filters {
                match filter {
                    AttributeFilter::Exact(value) => {
                        let term_value = json_value_to_term_string(value);
                        filters.push(format!(
                            "attributes @@@ '{}:{}'",
                            key.replace('\'', "''"),
                            term_value.replace('\'', "''")
                        ));
                    }
                    AttributeFilter::AnyOf(values) => {
                        let conditions: Vec<String> = values
                            .iter()
                            .map(|v| {
                                let term_value = json_value_to_term_string(v);
                                format!(
                                    "attributes @@@ '{}:{}'",
                                    key.replace('\'', "''"),
                                    term_value.replace('\'', "''")
                                )
                            })
                            .collect();
                        if !conditions.is_empty() {
                            filters.push(format!("({})", conditions.join(" OR ")));
                        }
                    }
                    AttributeFilter::Range { gte, lte } => {
                        if let Some(gte_val) = gte {
                            let gte_str = json_value_to_term_string(gte_val);
                            filters.push(format!(
                                "attributes->>'{}' >= '{}'",
                                key.replace('\'', "''"),
                                gte_str.replace('\'', "''")
                            ));
                        }
                        if let Some(lte_val) = lte {
                            let lte_str = json_value_to_term_string(lte_val);
                            filters.push(format!(
                                "attributes->>'{}' <= '{}'",
                                key.replace('\'', "''"),
                                lte_str.replace('\'', "''")
                            ));
                        }
                    }
                }
            }
        }

        if let Some(df) = date_filter {
            if let Some(after) = &df.after {
                let iso = after
                    .format(&time::format_description::well_known::Rfc3339)
                    .unwrap_or_default();
                filters.push(format!(
                    "metadata->>'updated_at' >= '{}'",
                    iso.replace('\'', "''")
                ));
            }
            if let Some(before) = &df.before {
                let iso = before
                    .format(&time::format_description::well_known::Rfc3339)
                    .unwrap_or_default();
                filters.push(format!(
                    "metadata->>'updated_at' <= '{}'",
                    iso.replace('\'', "''")
                ));
            }
        }

        if let Some(email) = user_email {
            filters.push(self.generate_permission_filter(email));
        }
    }

    pub async fn search(
        &self,
        query: &str,
        source_ids: &[String],
        content_types: Option<&[String]>,
        attribute_filters: Option<&HashMap<String, AttributeFilter>>,
        limit: i64,
        offset: i64,
        user_email: Option<&str>,
        document_id: Option<&str>,
        date_filter: Option<&DateFilter>,
        person_filters: Option<&[String]>,
        recency_boost_weight: f32,
        recency_half_life_days: f32,
    ) -> Result<Vec<SearchHit>, DatabaseError> {
        if source_ids.is_empty() {
            return Ok(vec![]);
        }

        // Filter-only search: no BM25 scoring, just apply filters sorted by recency
        if query.trim().is_empty() {
            return self
                .filter_only_search(
                    source_ids,
                    content_types,
                    attribute_filters,
                    limit,
                    offset,
                    user_email,
                    date_filter,
                )
                .await;
        }

        // We use term-centric ranking
        // Score each query term independently across fields (best-field per term),
        // then SUM across terms. This rewards docs matching MORE query terms.
        // Phrase bonus is additive on top.

        // Tokenize query via ParadeDB: stems, removes stopwords, ASCII-folds
        // This matches the index tokenizer pipeline exactly.
        let raw_terms: Vec<String> = sqlx::query_scalar(
            "SELECT unnest($1::pdb.simple('stemmer=english', 'stopwords_language=english', 'ascii_folding=true')::text[])"
        )
        .bind(query)
        .fetch_all(&self.pool)
        .await?;

        let mut seen = HashSet::new();
        let terms: Vec<String> = raw_terms
            .into_iter()
            .filter(|t| seen.insert(t.clone()))
            .take(8)
            .collect();

        // Bind params: $1 = full query, $2..$(1+N) = individual terms, then filters
        let mut param_idx = 2 + terms.len();

        let mut filters = Vec::new();
        self.build_common_filters(
            &mut filters,
            &mut param_idx,
            source_ids,
            content_types,
            attribute_filters,
            user_email,
            date_filter,
        );

        if document_id.is_some() {
            filters.push(format!("id = ${}", param_idx));
            param_idx += 1;
        }

        // Person filters: strict author filtering via BM25 index on metadata
        if let Some(persons) = person_filters {
            let conditions: Vec<String> = persons
                .iter()
                .map(|p| {
                    let escaped = p.replace('\'', "''");
                    format!("metadata ||| 'author:{escaped}'")
                })
                .collect();
            if !conditions.is_empty() {
                filters.push(format!("({})", conditions.join(" OR ")));
            }
        }

        let common_where = if filters.is_empty() {
            String::new()
        } else {
            format!(" AND {}", filters.join(" AND "))
        };

        // Per-term: best of title (default tokenizer), title (source_code tokenizer to handle
        // CamelCase), and content
        let mut term_branches = Vec::new();
        for (i, _term) in terms.iter().enumerate() {
            let term_param = format!("${}", 2 + i);
            term_branches.push(format!(
                "SELECT id, MAX(score) as score FROM (\
                    SELECT id, pdb.score(id) as score FROM documents \
                    WHERE title ||| {term_param}::pdb.boost(2){common_where} \
                    UNION ALL \
                    SELECT id, pdb.score(id) as score FROM documents \
                    WHERE title::pdb.alias('title_secondary') ||| {term_param}::pdb.boost(2){common_where} \
                    UNION ALL \
                    SELECT id, pdb.score(id) as score FROM documents \
                    WHERE content ||| {term_param}{common_where}\
                ) t{i} GROUP BY id"
            ));
        }

        // Phrase branches: best of title phrase vs content phrase (using $1 = full query)
        let phrase_branch = format!(
            "SELECT id, MAX(score) as score FROM (\
                SELECT id, pdb.score(id) as score FROM documents \
                WHERE title ### $1::pdb.slop(2)::pdb.boost(10){common_where} \
                UNION ALL \
                SELECT id, pdb.score(id) as score FROM documents \
                WHERE content ### $1::pdb.slop(2)::pdb.boost(5){common_where}\
            ) p GROUP BY id"
        );

        // When all query terms are stopwords, terms is empty — skip term_scores,
        // rank by phrase scoring only.
        let weight_idx = param_idx + 2;
        let half_life_idx = param_idx + 3;

        let recency_expr = format!(
            "(1.0 + ${w}::double precision * EXP(-EXTRACT(EPOCH FROM (CURRENT_TIMESTAMP - COALESCE(\
                CASE WHEN d.metadata->>'updated_at' IS NOT NULL \
                     AND pg_input_is_valid(d.metadata->>'updated_at', 'timestamptz') \
                THEN (d.metadata->>'updated_at')::timestamptz END, \
                d.updated_at))) / (86400.0 * ${h}::double precision)))::real",
            w = weight_idx,
            h = half_life_idx,
        );

        let full_query = if terms.is_empty() {
            format!(
                r#"
                WITH phrase_scores AS (
                    {phrase_branch}
                ),
                ranked AS (
                    SELECT ps.id, (ps.score * {recency_expr}) as score
                    FROM phrase_scores ps
                    JOIN documents d ON d.id = ps.id
                    ORDER BY score DESC
                    LIMIT ${limit_idx} OFFSET ${offset_idx}
                )
                SELECT r.id, r.score,
                       d.source_id, d.external_id, d.title, d.content_id, d.content_type,
                       d.file_size, d.file_extension, d.url,
                       d.metadata, d.permissions, d.attributes, d.created_at, d.updated_at, d.last_indexed_at,
                       COALESCE(snip.content_snippets, ARRAY[LEFT(d.content, 240)]) as content_snippets
                FROM ranked r
                JOIN documents d ON d.id = r.id
                LEFT JOIN LATERAL (
                    SELECT pdb.snippets(doc.content, start_tag => '**', end_tag => '**',
                                        max_num_chars => 200, "limit" => 3, sort_by => 'score') as content_snippets
                    FROM documents doc
                    WHERE doc.content ||| $1 AND doc.id = r.id
                    LIMIT 1
                ) snip ON true
                ORDER BY r.score DESC"#,
                phrase_branch = phrase_branch,
                recency_expr = recency_expr,
                limit_idx = param_idx,
                offset_idx = param_idx + 1,
            )
        } else {
            format!(
                r#"
                WITH term_scores AS (
                    {term_union}
                ),
                phrase_scores AS (
                    {phrase_branch}
                ),
                combined AS (
                    SELECT id, SUM(score) as token_score FROM term_scores GROUP BY id
                ),
                ranked AS (
                    SELECT c.id, ((c.token_score + COALESCE(p.score, 0)) * {recency_expr}) as score
                    FROM combined c
                    LEFT JOIN phrase_scores p ON c.id = p.id
                    JOIN documents d ON d.id = c.id
                    ORDER BY score DESC
                    LIMIT ${limit_idx} OFFSET ${offset_idx}
                )
                SELECT r.id, r.score,
                       d.source_id, d.external_id, d.title, d.content_id, d.content_type,
                       d.file_size, d.file_extension, d.url,
                       d.metadata, d.permissions, d.attributes, d.created_at, d.updated_at, d.last_indexed_at,
                       COALESCE(snip.content_snippets, ARRAY[LEFT(d.content, 240)]) as content_snippets
                FROM ranked r
                JOIN documents d ON d.id = r.id
                LEFT JOIN LATERAL (
                    SELECT pdb.snippets(doc.content, start_tag => '**', end_tag => '**',
                                        max_num_chars => 200, "limit" => 3, sort_by => 'score') as content_snippets
                    FROM documents doc
                    WHERE doc.content ||| $1 AND doc.id = r.id
                    LIMIT 1
                ) snip ON true
                ORDER BY r.score DESC"#,
                term_union = term_branches.join("\nUNION ALL\n"),
                phrase_branch = phrase_branch,
                recency_expr = recency_expr,
                limit_idx = param_idx,
                offset_idx = param_idx + 1,
            )
        };
        debug!("Full search query: {}", full_query);

        let mut query_builder = sqlx::query_as::<_, SearchHit>(&full_query).bind(query);

        for term in &terms {
            query_builder = query_builder.bind(term.as_str());
        }

        query_builder = query_builder.bind(source_ids);

        if let Some(ct) = content_types {
            if !ct.is_empty() {
                query_builder = query_builder.bind(ct);
            }
        }

        if let Some(doc_id) = document_id {
            query_builder = query_builder.bind(doc_id);
        }

        query_builder = query_builder
            .bind(limit)
            .bind(offset)
            .bind(recency_boost_weight as f64)
            .bind(recency_half_life_days as f64);

        let results = query_builder.fetch_all(&self.pool).await?;

        Ok(results)
    }

    async fn filter_only_search(
        &self,
        source_ids: &[String],
        content_types: Option<&[String]>,
        attribute_filters: Option<&HashMap<String, AttributeFilter>>,
        limit: i64,
        offset: i64,
        user_email: Option<&str>,
        date_filter: Option<&DateFilter>,
    ) -> Result<Vec<SearchHit>, DatabaseError> {
        let mut param_idx = 1;
        let mut filters = Vec::new();
        self.build_common_filters(
            &mut filters,
            &mut param_idx,
            source_ids,
            content_types,
            attribute_filters,
            user_email,
            date_filter,
        );

        let where_clause = if filters.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", filters.join(" AND "))
        };

        let query_str = format!(
            r#"
            SELECT id, 0.0::real as score, source_id, external_id, title, content_id, content_type,
                   file_size, file_extension, url,
                   metadata, permissions, attributes, created_at, updated_at, last_indexed_at,
                   ARRAY[LEFT(content, 240)] as content_snippets
            FROM documents
            {where_clause}
            ORDER BY updated_at DESC
            LIMIT ${limit_idx} OFFSET ${offset_idx}
            "#,
            where_clause = where_clause,
            limit_idx = param_idx,
            offset_idx = param_idx + 1,
        );

        let mut query_builder = sqlx::query_as::<_, SearchHit>(&query_str);

        query_builder = query_builder.bind(source_ids);

        if let Some(ct) = content_types {
            if !ct.is_empty() {
                query_builder = query_builder.bind(ct);
            }
        }

        query_builder = query_builder.bind(limit).bind(offset);

        let results = query_builder.fetch_all(&self.pool).await?;
        Ok(results)
    }

    pub async fn find_by_source(&self, source_id: &str) -> Result<Vec<Document>, DatabaseError> {
        let documents = sqlx::query_as::<_, Document>(
            r#"
            SELECT id, source_id, external_id, title, content_id, content_type,
                   file_size, file_extension, url,
                   metadata, permissions, attributes, created_at, updated_at, last_indexed_at
            FROM documents
            WHERE source_id = $1
            ORDER BY created_at DESC
            "#,
        )
        .bind(source_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(documents)
    }

    pub async fn find_by_external_id(
        &self,
        source_id: &str,
        external_id: &str,
    ) -> Result<Option<Document>, DatabaseError> {
        let document = sqlx::query_as::<_, Document>(
            r#"
            SELECT id, source_id, external_id, title, content_id, content_type,
                   file_size, file_extension, url,
                   metadata, permissions, attributes, created_at, updated_at, last_indexed_at
            FROM documents
            WHERE source_id = $1 AND external_id = $2
            "#,
        )
        .bind(source_id)
        .bind(external_id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(document)
    }

    pub async fn create(&self, document: Document) -> Result<Document, DatabaseError> {
        let created_document = sqlx::query_as::<_, Document>(
            r#"
            INSERT INTO documents (id, source_id, external_id, title, content_id, content_type, metadata, permissions, attributes)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
            RETURNING id, source_id, external_id, title, content_id, content_type,
                      file_size, file_extension, url,
                      metadata, permissions, attributes, created_at, updated_at, last_indexed_at
            "#
        )
        .bind(&document.id)
        .bind(&document.source_id)
        .bind(&document.external_id)
        .bind(&document.title)
        .bind(&document.content_id)
        .bind(&document.content_type)
        .bind(&document.metadata)
        .bind(&document.permissions)
        .bind(&document.attributes)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| match e {
            sqlx::Error::Database(db_err) if db_err.is_unique_violation() => {
                DatabaseError::ConstraintViolation("Document with this external_id already exists for this source".to_string())
            }
            _ => DatabaseError::from(e),
        })?;

        Ok(created_document)
    }

    /// Directly populates content to use the BM25 index
    pub async fn update(
        &self,
        id: &str,
        document: Document,
        content: &str,
    ) -> Result<Option<Document>, DatabaseError> {
        let updated_document = sqlx::query_as::<_, Document>(
            r#"
            UPDATE documents
            SET
                title = $2,
                content_id = $3,
                metadata = $4,
                permissions = $5,
                attributes = $6,
                content = $7
            WHERE id = $1
            RETURNING id, source_id, external_id, title, content_id, content_type,
                      file_size, file_extension, url,
                      metadata, permissions, attributes, created_at, updated_at, last_indexed_at
            "#,
        )
        .bind(id)
        .bind(&document.title)
        .bind(&document.content_id)
        .bind(&document.metadata)
        .bind(&document.permissions)
        .bind(&document.attributes)
        .bind(content)
        .fetch_optional(&self.pool)
        .await?;

        Ok(updated_document)
    }

    /// Partial update using COALESCE — only overwrites fields that are Some
    pub async fn update_fields(
        &self,
        id: &str,
        title: Option<&str>,
        content_id: Option<&str>,
        metadata: Option<&JsonValue>,
        permissions: Option<&JsonValue>,
    ) -> Result<Option<Document>, DatabaseError> {
        let updated_document = sqlx::query_as::<_, Document>(
            r#"
            UPDATE documents
            SET title = COALESCE($2, title),
                content_id = COALESCE($3, content_id),
                metadata = COALESCE($4, metadata),
                permissions = COALESCE($5, permissions),
                updated_at = $6
            WHERE id = $1
            RETURNING id, source_id, external_id, title, content_id, content_type,
                      file_size, file_extension, url,
                      metadata, permissions, attributes, created_at, updated_at, last_indexed_at
            "#,
        )
        .bind(id)
        .bind(title)
        .bind(content_id)
        .bind(metadata)
        .bind(permissions)
        .bind(sqlx::types::time::OffsetDateTime::now_utc())
        .fetch_optional(&self.pool)
        .await?;

        Ok(updated_document)
    }

    pub async fn delete(&self, id: &str) -> Result<bool, DatabaseError> {
        let result = sqlx::query("DELETE FROM documents WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;

        Ok(result.rows_affected() > 0)
    }

    /// Upserts a document with content for BM25 indexing
    pub async fn upsert(
        &self,
        document: Document,
        content: &str,
    ) -> Result<Document, DatabaseError> {
        let upserted_document = sqlx::query_as::<_, Document>(
            r#"
            INSERT INTO documents (id, source_id, external_id, title, content_id, content_type, file_size, file_extension, url, metadata, permissions, attributes, created_at, updated_at, last_indexed_at, content)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16)
            ON CONFLICT (source_id, external_id)
            DO UPDATE SET
                title = EXCLUDED.title,
                content_id = EXCLUDED.content_id,
                metadata = EXCLUDED.metadata,
                permissions = EXCLUDED.permissions,
                attributes = EXCLUDED.attributes,
                updated_at = EXCLUDED.updated_at,
                last_indexed_at = CURRENT_TIMESTAMP,
                content = EXCLUDED.content
            RETURNING id, source_id, external_id, title, content_id, content_type,
                      file_size, file_extension, url,
                      metadata, permissions, attributes, created_at, updated_at, last_indexed_at
            "#
        )
        .bind(&document.id)
        .bind(&document.source_id)
        .bind(&document.external_id)
        .bind(&document.title)
        .bind(&document.content_id)
        .bind(&document.content_type)
        .bind(&document.file_size)
        .bind(&document.file_extension)
        .bind(&document.url)
        .bind(&document.metadata)
        .bind(&document.permissions)
        .bind(&document.attributes)
        .bind(&document.created_at)
        .bind(&document.updated_at)
        .bind(&document.last_indexed_at)
        .bind(content)
        .fetch_one(&self.pool)
        .await?;

        Ok(upserted_document)
    }

    // TODO: This uses field-centric scoring (full query per field, MAX) while search() uses
    // term-centric scoring (per-term best-field, SUM). This can produce inconsistent facet counts.
    // Unify with search() scoring — either via a single combined query or shared scoring CTEs.
    pub async fn get_facet_counts(
        &self,
        query: &str,
        source_ids: &[String],
        content_types: Option<&[String]>,
        attribute_filters: Option<&HashMap<String, AttributeFilter>>,
        user_email: Option<&str>,
        date_filter: Option<&DateFilter>,
    ) -> Result<Vec<Facet>, DatabaseError> {
        if source_ids.is_empty() {
            return Ok(vec![]);
        }

        let mut filters = Vec::new();
        let mut param_idx = 2;

        self.build_common_filters(
            &mut filters,
            &mut param_idx,
            source_ids,
            content_types,
            attribute_filters,
            user_email,
            date_filter,
        );

        let common_where = if filters.is_empty() {
            String::new()
        } else {
            format!(" AND {}", filters.join(" AND "))
        };

        let union_branches = [
            format!("SELECT id, pdb.score(id) as score FROM documents WHERE title ||| $1::pdb.boost(2){common_where}"),
            format!("SELECT id, pdb.score(id) as score FROM documents WHERE content ||| $1{common_where}"),
            format!("SELECT id, pdb.score(id) as score FROM documents WHERE title::pdb.alias('title_secondary') ||| $1{common_where}"),
            format!("SELECT id, pdb.score(id) as score FROM documents WHERE title ### $1::pdb.slop(2)::pdb.boost(10){common_where}"),
            format!("SELECT id, pdb.score(id) as score FROM documents WHERE content ### $1::pdb.slop(2)::pdb.boost(5){common_where}"),
        ];

        let query_str = format!(
            r#"
            WITH field_scores AS (
                {union_all}
            ),
            best AS (
                SELECT id, MAX(score) as score
                FROM field_scores
                GROUP BY id
            ),
            thresholded AS (
                SELECT id FROM best
                WHERE score >= (SELECT MAX(score) FROM best) * 0.15
            )
            SELECT 'source_type' as facet, s.source_type as value, count(*) as count
            FROM thresholded t
            JOIN documents d ON d.id = t.id
            JOIN sources s ON d.source_id = s.id
            GROUP BY s.source_type
            ORDER BY count DESC
            "#,
            union_all = union_branches.join("\n                UNION ALL\n                "),
        );

        let mut query_builder = sqlx::query_as::<_, (String, String, i64)>(&query_str)
            .bind(query)
            .bind(source_ids);

        if let Some(ct) = content_types {
            if !ct.is_empty() {
                query_builder = query_builder.bind(ct);
            }
        }

        let facet_rows = query_builder.fetch_all(&self.pool).await?;

        let mut facets_map: std::collections::HashMap<String, Vec<FacetValue>> =
            std::collections::HashMap::new();

        for (facet_name, value, count) in facet_rows {
            facets_map
                .entry(facet_name)
                .or_insert_with(Vec::new)
                .push(FacetValue { value, count });
        }

        let facets: Vec<Facet> = facets_map
            .into_iter()
            .map(|(name, values)| Facet { name, values })
            .collect();

        Ok(facets)
    }

    /// Directly populates the content field since we use the ParadeDB BM25 index now
    pub async fn batch_upsert(
        &self,
        documents: Vec<Document>,
        contents: Vec<String>,
    ) -> Result<Vec<Document>, DatabaseError> {
        if documents.is_empty() {
            return Ok(vec![]);
        }

        // Build arrays for the batch upsert
        let ids: Vec<String> = documents.iter().map(|d| d.id.clone()).collect();
        let source_ids: Vec<String> = documents.iter().map(|d| d.source_id.clone()).collect();
        let external_ids: Vec<String> = documents.iter().map(|d| d.external_id.clone()).collect();
        let titles: Vec<String> = documents.iter().map(|d| d.title.clone()).collect();
        let content_ids: Vec<Option<String>> =
            documents.iter().map(|d| d.content_id.clone()).collect();
        let content_types: Vec<Option<String>> =
            documents.iter().map(|d| d.content_type.clone()).collect();
        let file_sizes: Vec<Option<i64>> = documents.iter().map(|d| d.file_size).collect();
        let file_extensions: Vec<Option<String>> =
            documents.iter().map(|d| d.file_extension.clone()).collect();
        let urls: Vec<Option<String>> = documents.iter().map(|d| d.url.clone()).collect();
        let metadata: Vec<serde_json::Value> =
            documents.iter().map(|d| d.metadata.clone()).collect();
        let permissions: Vec<serde_json::Value> =
            documents.iter().map(|d| d.permissions.clone()).collect();
        let attributes: Vec<serde_json::Value> =
            documents.iter().map(|d| d.attributes.clone()).collect();
        let created_ats: Vec<sqlx::types::time::OffsetDateTime> =
            documents.iter().map(|d| d.created_at).collect();
        let updated_ats: Vec<sqlx::types::time::OffsetDateTime> =
            documents.iter().map(|d| d.updated_at).collect();
        let last_indexed_ats: Vec<sqlx::types::time::OffsetDateTime> =
            documents.iter().map(|d| d.last_indexed_at).collect();

        let upserted_documents = sqlx::query_as::<_, Document>(
            r#"
            INSERT INTO documents (
                id,
                source_id,
                external_id,
                title,
                content_id,
                content_type,
                file_size,
                file_extension,
                url,
                metadata,
                permissions,
                attributes,
                created_at,
                updated_at,
                last_indexed_at,
                content
            )
            SELECT *
            FROM UNNEST(
                $1::text[], $2::text[], $3::text[], $4::text[], $5::text[], $6::text[],
                $7::bigint[], $8::text[], $9::text[], $10::jsonb[], $11::jsonb[], $12::jsonb[],
                $13::timestamptz[], $14::timestamptz[], $15::timestamptz[], $16::text[]
            ) AS t(id, source_id, external_id, title, content_id, content_type, file_size, file_extension, url, metadata, permissions, attributes, created_at, updated_at, last_indexed_at, content)
            ON CONFLICT (source_id, external_id)
            DO UPDATE SET
                title = EXCLUDED.title,
                content_id = EXCLUDED.content_id,
                metadata = EXCLUDED.metadata,
                permissions = EXCLUDED.permissions,
                attributes = EXCLUDED.attributes,
                updated_at = EXCLUDED.updated_at,
                last_indexed_at = CURRENT_TIMESTAMP,
                content = EXCLUDED.content
            RETURNING id, source_id, external_id, title, content_id, content_type,
                      file_size, file_extension, url,
                      metadata, permissions, attributes, created_at, updated_at, last_indexed_at
            "#
        )
        .bind(&ids)
        .bind(&source_ids)
        .bind(&external_ids)
        .bind(&titles)
        .bind(&content_ids)
        .bind(&content_types)
        .bind(&file_sizes)
        .bind(&file_extensions)
        .bind(&urls)
        .bind(&metadata)
        .bind(&permissions)
        .bind(&attributes)
        .bind(&created_ats)
        .bind(&updated_ats)
        .bind(&last_indexed_ats)
        .bind(&contents)
        .fetch_all(&self.pool)
        .await?;

        Ok(upserted_documents)
    }

    pub async fn batch_delete(&self, document_ids: Vec<String>) -> Result<i64, DatabaseError> {
        if document_ids.is_empty() {
            return Ok(0);
        }

        let result = sqlx::query("DELETE FROM documents WHERE id = ANY($1)")
            .bind(&document_ids)
            .execute(&self.pool)
            .await?;

        Ok(result.rows_affected() as i64)
    }
}

/// Convert a JSON value to a string suitable for ParadeDB term queries
fn json_value_to_term_string(value: &JsonValue) -> String {
    match value {
        JsonValue::String(s) => s.clone(),
        JsonValue::Number(n) => n.to_string(),
        JsonValue::Bool(b) => b.to_string(),
        JsonValue::Null => "null".to_string(),
        // For arrays and objects, serialize to JSON string
        _ => value.to_string(),
    }
}
