use serde_json::Value as JsonValue;
use shared::{
    db::error::DatabaseError,
    db::repositories::document,
    models::{AttributeFilter, DateFilter, Document, Facet, FacetValue},
};
use sqlx::{postgres::PgRow, FromRow, PgPool, Row};
use std::collections::{HashMap, HashSet};
use tracing::debug;

/// Extra candidates fetched beyond offset+limit so that recency re-ranking
/// doesn't miss relevant results.
const CANDIDATE_PADDING: i64 = 200;

/// Maximum candidates considered for facet counts. TopN pushes this limit into
/// the Tantivy index scan, avoiding full result-set materialisation.
const FACET_CANDIDATE_LIMIT: i64 = 10_000;

/// Drop weak fulltext matches relative to the strongest recency-adjusted score.
/// Keep this in SQL so `total_count` and pagination use the same row universe
/// as displayed fulltext hits.
const MIN_SCORE_RATIO: f32 = 0.15;

#[derive(FromRow)]
pub struct SearchHit {
    #[sqlx(flatten)]
    pub document: Document,
    pub score: f32,
    #[sqlx(default)]
    pub content_snippets: Option<Vec<String>>,
    #[sqlx(default)]
    pub source_type: Option<String>,
}

struct SearchHitWithTotalRow {
    hit: Option<SearchHit>,
    total_count: i64,
}

impl<'r> FromRow<'r, PgRow> for SearchHitWithTotalRow {
    fn from_row(row: &'r PgRow) -> Result<Self, sqlx::Error> {
        let total_count = row.try_get("total_count")?;
        let id: Option<String> = row.try_get("id")?;
        let hit = if id.is_some() {
            Some(SearchHit::from_row(row)?)
        } else {
            None
        };

        Ok(Self { hit, total_count })
    }
}

impl SearchHitWithTotalRow {
    fn into_search_hit(self) -> Option<SearchHit> {
        self.hit
    }
}

pub struct SearchDocumentRepository {
    pool: PgPool,
}

impl SearchDocumentRepository {
    pub fn new(pool: &PgPool) -> Self {
        Self { pool: pool.clone() }
    }

    pub async fn build_query_text(&self, query: &str) -> Result<Option<String>, DatabaseError> {
        if query.trim().is_empty() {
            return Ok(None);
        }

        // Tokenize query via ParadeDB: splits on non-alphanumeric, ASCII-folds.
        // No stemming or stopwords — dropping stopwords would remove valid words
        // in non-English languages (e.g. German "die", "in", "was").
        let raw_terms: Vec<String> =
            sqlx::query_scalar("SELECT unnest($1::pdb.simple('ascii_folding=true')::text[])")
                .bind(query)
                .fetch_all(&self.pool)
                .await?;

        let mut seen = HashSet::new();
        // Cap at 12 terms. Without stopword removal longer queries produce more
        // tokens than before. Each term adds field-boosted clauses to the Tantivy
        // query string, so this keeps query complexity bounded.
        let terms: Vec<String> = raw_terms
            .into_iter()
            .filter(|t| seen.insert(t.clone()))
            .take(12)
            .collect();

        Ok(Some(build_tantivy_query(&terms, query)))
    }

    pub async fn search(
        &self,
        query: &str,
        tantivy_query: Option<&str>,
        source_ids: &[String],
        content_types: Option<&[String]>,
        attribute_filters: Option<&HashMap<String, AttributeFilter>>,
        limit: i64,
        offset: i64,
        user_email: Option<&str>,
        user_groups: &[String],
        document_id: Option<&str>,
        date_filter: Option<&DateFilter>,
        person_filters: Option<&[String]>,
        recency_boost_weight: f32,
        recency_half_life_days: f32,
    ) -> Result<(Vec<SearchHit>, i64), DatabaseError> {
        if source_ids.is_empty() {
            return Ok((vec![], 0));
        }

        if query.trim().is_empty() {
            return self
                .filter_only_search(
                    source_ids,
                    content_types,
                    attribute_filters,
                    limit,
                    offset,
                    user_email,
                    user_groups,
                    date_filter,
                    person_filters,
                )
                .await;
        }

        let owned_tantivy_query;
        let tantivy_query = if let Some(tq) = tantivy_query {
            tq
        } else {
            owned_tantivy_query = self.build_query_text(query).await?.unwrap_or_default();
            &owned_tantivy_query
        };

        // Bind params: $1 = tantivy query string, then filters
        let mut param_idx = 2;

        let mut filters = Vec::new();
        build_common_filters(
            &mut filters,
            &mut param_idx,
            source_ids,
            content_types,
            attribute_filters,
            user_email,
            user_groups,
            date_filter,
        );

        if document_id.is_some() {
            filters.push(format!("d.id = ${}", param_idx));
            param_idx += 1;
        }

        // Person filters: strict author filtering via BM25 index on metadata
        if let Some(persons) = person_filters {
            let conditions: Vec<String> = persons
                .iter()
                .map(|p| {
                    let escaped = p.replace('\'', "''");
                    format!("d.metadata ||| 'author:{escaped}'")
                })
                .collect();
            if !conditions.is_empty() {
                filters.push(format!("({})", conditions.join(" OR ")));
            }
        }

        let filter_where = if filters.is_empty() {
            String::new()
        } else {
            format!(" AND {}", filters.join(" AND "))
        };

        // Bind order: $1=tantivy_query, filters..., candidate_limit, limit,
        // offset, recency_weight, recency_half_life
        let candidate_limit_idx = param_idx;
        let limit_idx = param_idx + 1;
        let offset_idx = param_idx + 2;
        let weight_idx = param_idx + 3;
        let half_life_idx = param_idx + 4;
        let min_score_ratio_idx = param_idx + 5;

        let recency_expr = format!(
            "(1.0 + ${w}::double precision * EXP(-EXTRACT(EPOCH FROM (CURRENT_TIMESTAMP - COALESCE(\
                CASE WHEN d.metadata->>'updated_at' IS NOT NULL \
                     AND pg_input_is_valid(d.metadata->>'updated_at', 'timestamptz') \
                THEN (d.metadata->>'updated_at')::timestamptz END, \
                d.updated_at))) / (86400.0 * ${h}::double precision)))::real",
            w = weight_idx,
            h = half_life_idx,
        );

        let full_query = format!(
            r#"
            WITH filtered_candidates AS MATERIALIZED (
                SELECT d.id, d.source_id, pdb.score(d.id) as bm25_score
                FROM documents d
                JOIN sources s ON s.id = d.source_id AND NOT s.is_deleted
                WHERE d.id @@@ pdb.parse($1, lenient => true){filter_where}
                ORDER BY bm25_score DESC
                LIMIT ${candidate_limit_idx}
            ),
            scored_candidates AS (
                SELECT fc.id, (fc.bm25_score * {recency_expr}) as score
                FROM filtered_candidates fc
                JOIN documents d ON d.id = fc.id
            ),
            max_score AS (
                SELECT MAX(score) AS value FROM scored_candidates
            ),
            relevant_candidates AS MATERIALIZED (
                SELECT sc.id, sc.score
                FROM scored_candidates sc
                CROSS JOIN max_score ms
                WHERE ms.value <= 0 OR sc.score >= (ms.value * ${min_score_ratio_idx}::real)
            ),
            -- Dedupe by (source_type, external_id), not source_id. Connectors may emit
            -- the same logical document from multiple sources of the same type (for
            -- example two IMAP accounts seeing the same thread), but external_id is
            -- only considered canonical within a connector/source-type namespace.
            deduped_candidates AS MATERIALIZED (
                SELECT id, score
                FROM (
                    SELECT rc.id, rc.score,
                           ROW_NUMBER() OVER (
                               PARTITION BY s.source_type, d.external_id
                               ORDER BY rc.score DESC,
                                        COALESCE(
                                            CASE WHEN d.metadata->>'updated_at' IS NOT NULL
                                                 AND pg_input_is_valid(d.metadata->>'updated_at', 'timestamptz')
                                            THEN (d.metadata->>'updated_at')::timestamptz END,
                                            d.updated_at) DESC,
                                        d.id
                           ) AS dedupe_rank
                    FROM relevant_candidates rc
                    JOIN documents d ON d.id = rc.id
                    JOIN sources s ON s.id = d.source_id AND NOT s.is_deleted
                ) ranked_candidates
                WHERE dedupe_rank = 1
            ),
            total AS (
                SELECT COUNT(*)::bigint AS total_count FROM deduped_candidates
            ),
            ranked AS (
                SELECT dc.id, dc.score
                FROM deduped_candidates dc
                ORDER BY score DESC
                LIMIT ${limit_idx} OFFSET ${offset_idx}
            ),
            hits AS (
                SELECT r.id, r.score,
                       d.source_id, d.external_id, d.title, d.content_id, d.content_type,
                       d.file_size, d.file_extension, d.url,
                       d.metadata, d.permissions, d.attributes, d.created_at, d.updated_at, d.last_indexed_at,
                       NULL::text[] as content_snippets,
                       s.source_type::text as source_type
                FROM ranked r
                JOIN documents d ON d.id = r.id
                JOIN sources s ON s.id = d.source_id AND NOT s.is_deleted
            )
            SELECT h.id, h.score,
                   h.source_id, h.external_id, h.title, h.content_id, h.content_type,
                   h.file_size, h.file_extension, h.url,
                   h.metadata, h.permissions, h.attributes, h.created_at, h.updated_at, h.last_indexed_at,
                   h.content_snippets, h.source_type,
                   t.total_count
            FROM total t
            LEFT JOIN hits h ON TRUE
            ORDER BY h.score DESC NULLS LAST"#,
            filter_where = filter_where,
            recency_expr = recency_expr,
            candidate_limit_idx = candidate_limit_idx,
            limit_idx = limit_idx,
            offset_idx = offset_idx,
            min_score_ratio_idx = min_score_ratio_idx,
        );
        debug!("Full search query: {}", full_query);

        let mut query_builder =
            sqlx::query_as::<_, SearchHitWithTotalRow>(&full_query).bind(tantivy_query);

        query_builder = query_builder.bind(source_ids);

        if let Some(ct) = content_types {
            if !ct.is_empty() {
                query_builder = query_builder.bind(ct);
            }
        }

        if let Some(doc_id) = document_id {
            query_builder = query_builder.bind(doc_id);
        }

        let candidate_limit = FACET_CANDIDATE_LIMIT.max(offset + limit + CANDIDATE_PADDING);
        query_builder = query_builder
            .bind(candidate_limit)
            .bind(limit)
            .bind(offset)
            .bind(recency_boost_weight as f64)
            .bind(recency_half_life_days as f64)
            .bind(MIN_SCORE_RATIO);

        let rows = query_builder.fetch_all(&self.pool).await?;
        let total_count = rows.first().map_or(0, |row| row.total_count);
        let results = rows
            .into_iter()
            .filter_map(SearchHitWithTotalRow::into_search_hit)
            .collect();

        Ok((results, total_count))
    }

    async fn filter_only_search(
        &self,
        source_ids: &[String],
        content_types: Option<&[String]>,
        attribute_filters: Option<&HashMap<String, AttributeFilter>>,
        limit: i64,
        offset: i64,
        user_email: Option<&str>,
        user_groups: &[String],
        date_filter: Option<&DateFilter>,
        person_filters: Option<&[String]>,
    ) -> Result<(Vec<SearchHit>, i64), DatabaseError> {
        let mut param_idx = 1;
        let mut filters = Vec::new();
        build_common_filters(
            &mut filters,
            &mut param_idx,
            source_ids,
            content_types,
            attribute_filters,
            user_email,
            user_groups,
            date_filter,
        );

        // Apply person filters (from `by:Name` operators) here too — without
        // this, an empty-query browse with `by:Alice` silently ignores the
        // person filter and returns everything.
        //
        // Uses plain JSONB ILIKE instead of the `metadata ||| 'author:X'` BM25
        // operator because BM25 operators require a BM25 scoring context
        // (the `@@@` operator elsewhere in the query). In the filter-only path
        // there's no `@@@`, so BM25 operators are no-ops and every row matches.
        if let Some(persons) = person_filters {
            let conditions: Vec<String> = persons
                .iter()
                .map(|p| {
                    let escaped = p.replace('\'', "''");
                    format!("d.metadata->>'author' ILIKE '%{escaped}%'")
                })
                .collect();
            if !conditions.is_empty() {
                filters.push(format!("({})", conditions.join(" OR ")));
            }
        }

        let filter_where = if filters.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", filters.join(" AND "))
        };

        let query_str = format!(
            r#"
            WITH filtered_scope AS MATERIALIZED (
                SELECT d.id, d.source_id
                FROM documents d
                JOIN sources s ON s.id = d.source_id AND NOT s.is_deleted
                {filter_where}
            ),
            -- Dedupe by (source_type, external_id), not source_id. Connectors may emit
            -- the same logical document from multiple sources of the same type, but
            -- external_id is only considered canonical within a connector/source-type
            -- namespace, so different source types with matching IDs remain distinct.
            deduped_scope AS MATERIALIZED (
                SELECT id
                FROM (
                    SELECT fs.id,
                           ROW_NUMBER() OVER (
                               PARTITION BY s.source_type, d.external_id
                               ORDER BY COALESCE(
                                            CASE WHEN d.metadata->>'updated_at' IS NOT NULL
                                                 AND pg_input_is_valid(d.metadata->>'updated_at', 'timestamptz')
                                            THEN (d.metadata->>'updated_at')::timestamptz END,
                                            d.updated_at) DESC,
                                        d.id
                           ) AS dedupe_rank
                    FROM filtered_scope fs
                    JOIN documents d ON d.id = fs.id
                    JOIN sources s ON s.id = d.source_id AND NOT s.is_deleted
                ) ranked_scope
                WHERE dedupe_rank = 1
            ),
            total AS (
                SELECT COUNT(*)::bigint AS total_count FROM deduped_scope
            ),
            ranked AS (
                SELECT ds.id
                FROM deduped_scope ds
                JOIN documents d ON d.id = ds.id
                ORDER BY COALESCE(
                    CASE WHEN d.metadata->>'updated_at' IS NOT NULL
                         AND pg_input_is_valid(d.metadata->>'updated_at', 'timestamptz')
                    THEN (d.metadata->>'updated_at')::timestamptz END,
                    d.updated_at) DESC
                LIMIT ${limit_idx} OFFSET ${offset_idx}
            ),
            hits AS (
                SELECT d.id, 0.0::real as score, d.source_id, d.external_id, d.title, d.content_id, d.content_type,
                       d.file_size, d.file_extension, d.url,
                       d.metadata, d.permissions, d.attributes, d.created_at, d.updated_at, d.last_indexed_at,
                       ARRAY[LEFT(d.content, 240)] as content_snippets,
                       s.source_type::text as source_type
                FROM ranked r
                JOIN documents d ON d.id = r.id
                JOIN sources s ON s.id = d.source_id AND NOT s.is_deleted
            )
            SELECT h.id, h.score, h.source_id, h.external_id, h.title, h.content_id, h.content_type,
                   h.file_size, h.file_extension, h.url,
                   h.metadata, h.permissions, h.attributes, h.created_at, h.updated_at, h.last_indexed_at,
                   h.content_snippets, h.source_type,
                   t.total_count
            FROM total t
            LEFT JOIN hits h ON TRUE
            ORDER BY h.updated_at DESC NULLS LAST
            "#,
            filter_where = filter_where,
            limit_idx = param_idx,
            offset_idx = param_idx + 1,
        );

        let mut query_builder = sqlx::query_as::<_, SearchHitWithTotalRow>(&query_str);

        query_builder = query_builder.bind(source_ids);

        if let Some(ct) = content_types {
            if !ct.is_empty() {
                query_builder = query_builder.bind(ct);
            }
        }

        query_builder = query_builder.bind(limit).bind(offset);

        let rows = query_builder.fetch_all(&self.pool).await?;
        let total_count = rows.first().map_or(0, |row| row.total_count);
        let results = rows
            .into_iter()
            .filter_map(SearchHitWithTotalRow::into_search_hit)
            .collect();
        Ok((results, total_count))
    }

    pub async fn fetch_highlights(
        &self,
        document_ids: &[String],
        query: &str,
    ) -> Result<HashMap<String, Vec<String>>, DatabaseError> {
        if document_ids.is_empty() || query.trim().is_empty() {
            return Ok(HashMap::new());
        }

        let rows: Vec<(String, Option<Vec<String>>)> = sqlx::query_as(
            r#"
            SELECT id,
                   ARRAY[ts_headline('english', content,
                       plainto_tsquery('english', $2),
                       'StartSel=**, StopSel=**, MaxFragments=3, MaxWords=30, MinWords=10'
                   )] as content_snippets
            FROM documents
            WHERE id = ANY($1)
            "#,
        )
        .bind(document_ids)
        .bind(query)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .filter_map(|(id, snippets)| {
                let snippets = snippets?
                    .into_iter()
                    .filter(|snippet| !snippet.is_empty())
                    .collect::<Vec<String>>();
                if snippets.is_empty() {
                    None
                } else {
                    Some((id, snippets))
                }
            })
            .collect())
    }

    pub async fn get_facet_counts(
        &self,
        query: &str,
        source_ids: &[String],
        content_types: Option<&[String]>,
        attribute_filters: Option<&HashMap<String, AttributeFilter>>,
        user_email: Option<&str>,
        user_groups: &[String],
        date_filter: Option<&DateFilter>,
        person_filters: Option<&[String]>,
    ) -> Result<Vec<Facet>, DatabaseError> {
        let tantivy_query = self.build_query_text(query).await?;
        self.get_facet_counts_with_query_text(
            query,
            tantivy_query.as_deref(),
            source_ids,
            content_types,
            attribute_filters,
            user_email,
            user_groups,
            date_filter,
            person_filters,
        )
        .await
    }

    pub async fn get_facet_counts_with_query_text(
        &self,
        query: &str,
        tantivy_query: Option<&str>,
        source_ids: &[String],
        content_types: Option<&[String]>,
        attribute_filters: Option<&HashMap<String, AttributeFilter>>,
        user_email: Option<&str>,
        user_groups: &[String],
        date_filter: Option<&DateFilter>,
        person_filters: Option<&[String]>,
    ) -> Result<Vec<Facet>, DatabaseError> {
        if source_ids.is_empty() {
            return Ok(vec![]);
        }

        if query.trim().is_empty() {
            // No BM25 scoring possible — count all docs matching filters
            let mut param_idx = 1;
            let mut filters = Vec::new();
            build_common_filters(
                &mut filters,
                &mut param_idx,
                source_ids,
                content_types,
                attribute_filters,
                user_email,
                user_groups,
                date_filter,
            );
            let where_clause = if filters.is_empty() {
                String::new()
            } else {
                format!("WHERE {}", filters.join(" AND "))
            };
            let query_str = format!(
                r#"
                SELECT 'source_type' as facet, s.source_type as value, count(*) as count
                FROM documents d
                JOIN sources s ON d.source_id = s.id
                {where_clause}
                GROUP BY s.source_type
                ORDER BY count DESC
                "#,
            );
            let mut qb = sqlx::query_as::<_, (String, String, i64)>(&query_str).bind(source_ids);
            if let Some(ct) = content_types {
                if !ct.is_empty() {
                    qb = qb.bind(ct);
                }
            }
            let rows = qb.fetch_all(&self.pool).await?;
            return Ok(rows_to_facets(rows));
        }

        let owned_tantivy_query;
        let tantivy_query = if let Some(tq) = tantivy_query {
            tq
        } else {
            owned_tantivy_query = self.build_query_text(query).await?.unwrap_or_default();
            &owned_tantivy_query
        };

        // Bind params: $1 = tantivy query string, then filters
        let mut param_idx = 2;

        let mut filters = Vec::new();
        build_common_filters(
            &mut filters,
            &mut param_idx,
            source_ids,
            content_types,
            attribute_filters,
            user_email,
            user_groups,
            date_filter,
        );

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

        let filter_where = if filters.is_empty() {
            String::new()
        } else {
            format!(" AND {}", filters.join(" AND "))
        };

        let facet_limit_idx = param_idx;

        let query_str = format!(
            r#"
            WITH candidates AS (
                SELECT id, pdb.score(id) as score
                FROM documents
                WHERE id @@@ pdb.parse($1, lenient => true){filter_where}
                ORDER BY score DESC
                LIMIT ${facet_limit_idx}
            )
            SELECT 'source_type' as facet, s.source_type as value, count(*) as count
            FROM candidates c
            JOIN documents d ON d.id = c.id
            JOIN sources s ON d.source_id = s.id
            GROUP BY s.source_type
            ORDER BY count DESC
            "#,
            filter_where = filter_where,
            facet_limit_idx = facet_limit_idx,
        );

        let mut query_builder =
            sqlx::query_as::<_, (String, String, i64)>(&query_str).bind(tantivy_query);

        query_builder = query_builder.bind(source_ids);

        if let Some(ct) = content_types {
            if !ct.is_empty() {
                query_builder = query_builder.bind(ct);
            }
        }

        query_builder = query_builder.bind(FACET_CANDIDATE_LIMIT);

        let facet_rows = query_builder.fetch_all(&self.pool).await?;
        Ok(rows_to_facets(facet_rows))
    }

    pub async fn get_distinct_attribute_values(
        &self,
        keys: &[String],
        limit: i64,
    ) -> Result<HashMap<String, Vec<String>>, DatabaseError> {
        if keys.is_empty() {
            return Ok(HashMap::new());
        }

        let rows: Vec<(String, String)> = sqlx::query_as(
            r#"
            SELECT key, val FROM (
                SELECT
                    key,
                    val,
                    ROW_NUMBER() OVER (PARTITION BY key ORDER BY val) AS rn
                FROM (
                    SELECT DISTINCT k AS key, attributes->>k AS val
                    FROM documents, UNNEST($1::text[]) AS k
                    WHERE attributes ? k AND attributes->>k IS NOT NULL
                ) distinct_vals
            ) ranked
            WHERE rn <= $2
            ORDER BY key, val
            "#,
        )
        .bind(keys)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        let mut result: HashMap<String, Vec<String>> = HashMap::new();
        for (key, val) in rows {
            result.entry(key).or_default().push(val);
        }
        Ok(result)
    }
}

fn rows_to_facets(rows: Vec<(String, String, i64)>) -> Vec<Facet> {
    let mut facets_map: HashMap<String, Vec<FacetValue>> = HashMap::new();
    for (facet_name, value, count) in rows {
        facets_map.entry(facet_name).or_default().push(FacetValue {
            value,
            count: Some(count),
        });
    }
    facets_map
        .into_iter()
        .map(|(name, values)| Facet { name, values })
        .collect()
}

fn generate_permission_filter(user_email: &str, user_groups: &[String]) -> String {
    document::generate_permission_filter(user_email, user_groups)
}

// TODO: use tantivy crate for query string validation
fn build_tantivy_query(terms: &[String], original_query: &str) -> String {
    let mut clauses = Vec::new();

    for term in terms {
        let escaped = escape_tantivy_term(term);
        clauses.push(format!("title:{escaped}^2"));
        clauses.push(format!("title_secondary:{escaped}^2"));
        clauses.push(format!("title_en:{escaped}^2"));
        clauses.push(format!("content:{escaped}"));
        clauses.push(format!("content_en:{escaped}"));
    }

    // Phrase matching on the original query with slop and boost
    let escaped_phrase = original_query.replace('\\', "\\\\").replace('"', "\\\"");
    clauses.push(format!("title:\"{escaped_phrase}\"~2^10"));
    clauses.push(format!("title_en:\"{escaped_phrase}\"~2^10"));
    clauses.push(format!("content:\"{escaped_phrase}\"~2^5"));
    clauses.push(format!("content_en:\"{escaped_phrase}\"~2^5"));

    clauses.join(" ")
}

fn escape_tantivy_term(term: &str) -> String {
    let mut escaped = String::with_capacity(term.len());
    for ch in term.chars() {
        if matches!(
            ch,
            '+' | '-'
                | '('
                | ')'
                | '{'
                | '}'
                | '['
                | ']'
                | '^'
                | '"'
                | '~'
                | '*'
                | '?'
                | '\\'
                | '/'
                | ':'
        ) {
            escaped.push('\\');
        }
        escaped.push(ch);
    }
    escaped
}

fn json_value_to_term_string(value: &JsonValue) -> String {
    match value {
        JsonValue::String(s) => s.clone(),
        JsonValue::Number(n) => n.to_string(),
        JsonValue::Bool(b) => b.to_string(),
        JsonValue::Null => "null".to_string(),
        _ => value.to_string(),
    }
}

fn build_common_filters(
    filters: &mut Vec<String>,
    param_idx: &mut usize,
    source_ids: &[String],
    content_types: Option<&[String]>,
    attribute_filters: Option<&HashMap<String, AttributeFilter>>,
    user_email: Option<&str>,
    user_groups: &[String],
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
        filters.push(generate_permission_filter(email, user_groups));
    }
}
