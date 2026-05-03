use serde_json::Value as JsonValue;
use shared::{
    db::error::DatabaseError,
    db::repositories::document,
    models::{AttributeFilter, DateFilter, Document, Facet, FacetValue},
};
use sqlx::{FromRow, PgPool};
use std::collections::{HashMap, HashSet};
use tracing::debug;

/// Maximum candidates considered for any BM25 query path. TopN pushes this
/// limit into the Tantivy index scan, avoiding full result-set materialisation.
/// Shared by `models::SearchRequest::dedupe_fetch_limit`.
pub(crate) const CANDIDATE_LIMIT: i64 = 10_000;

/// Minimum BM25 score (as a fraction of the top match) that a result must
/// reach to be considered relevant before candidate results leave SQL.
pub(crate) const MIN_SCORE_RATIO: f64 = 0.15;

#[derive(FromRow)]
pub struct SearchHit {
    #[sqlx(flatten)]
    pub document: Document,
    pub score: f32,
    #[sqlx(default)]
    pub content_snippets: Option<Vec<String>>,
}

/// Tokenized query and pre-formatted filter clause shared by every BM25 query
/// path (search, count, facets). Caller binds in order: `$1 = tantivy_query`
/// (and optionally `$2 = original query` if `starting_param_idx` was 3 to
/// reserve a slot for ts_headline), then `source_ids`, optional `content_types`,
/// optional `document_id`, then any caller-specific tail (limit/offset/etc).
struct Bm25Filters {
    tantivy_query: String,
    /// Either empty or a `" AND <expr> AND ..."` fragment to splice into a
    /// SQL WHERE that already has a leading `id @@@ pdb.parse(...)` predicate.
    filter_where: String,
    /// Next placeholder index available for caller-specific binds
    /// (e.g. `LIMIT $N`).
    next_param_idx: usize,
}

pub struct SearchDocumentRepository {
    pool: PgPool,
}

impl SearchDocumentRepository {
    pub fn new(pool: &PgPool) -> Self {
        Self { pool: pool.clone() }
    }

    /// Tokenize the query and build the filter clause shared by all BM25
    /// query paths. `starting_param_idx` is the first placeholder caller
    /// reserves AFTER `$1 = tantivy_query` (and optional `$2 = original
    /// query` for ts_headline) — so 2 for count/facets, 3 for search.
    /// `include_document_id` toggles whether `document_id` is wired in
    /// (search and count support it; facets do not).
    async fn build_bm25_filters(
        &self,
        query: &str,
        source_ids: &[String],
        content_types: Option<&[String]>,
        attribute_filters: Option<&HashMap<String, AttributeFilter>>,
        user_email: Option<&str>,
        user_groups: &[String],
        document_id: Option<&str>,
        date_filter: Option<&DateFilter>,
        person_filters: Option<&[String]>,
        starting_param_idx: usize,
    ) -> Result<Bm25Filters, DatabaseError> {
        // Tokenize via ParadeDB: splits on non-alphanumeric, ASCII-folds.
        // No stemming or stopwords — dropping stopwords would remove valid
        // words in non-English languages (e.g. German "die", "in", "was").
        let raw_terms: Vec<String> =
            sqlx::query_scalar("SELECT unnest($1::pdb.simple('ascii_folding=true')::text[])")
                .bind(query)
                .fetch_all(&self.pool)
                .await?;

        let mut seen = HashSet::new();
        // Cap at 12 terms — without stopword removal, longer queries produce
        // more tokens, and each adds field-boosted clauses to the Tantivy
        // query string. Bounds query complexity.
        let terms: Vec<String> = raw_terms
            .into_iter()
            .filter(|t| seen.insert(t.clone()))
            .take(12)
            .collect();

        let tantivy_query = build_tantivy_query(&terms, query);

        let mut param_idx = starting_param_idx;
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
            filters.push(format!("id = ${param_idx}"));
            param_idx += 1;
        }

        // Person filters: strict author filtering via BM25 index on metadata.
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

        Ok(Bm25Filters {
            tantivy_query,
            filter_where,
            next_param_idx: param_idx,
        })
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
        user_groups: &[String],
        document_id: Option<&str>,
        date_filter: Option<&DateFilter>,
        person_filters: Option<&[String]>,
        recency_boost_weight: f32,
        recency_half_life_days: f32,
    ) -> Result<Vec<SearchHit>, DatabaseError> {
        if source_ids.is_empty() {
            return Ok(vec![]);
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

        // $1 = tantivy_query, $2 = original query (reserved for ts_headline),
        // $3+ = filter binds, then candidate_limit/limit/offset/recency.
        let Bm25Filters {
            tantivy_query,
            filter_where,
            next_param_idx,
        } = self
            .build_bm25_filters(
                query,
                source_ids,
                content_types,
                attribute_filters,
                user_email,
                user_groups,
                document_id,
                date_filter,
                person_filters,
                3,
            )
            .await?;

        let candidate_limit_idx = next_param_idx;
        let limit_idx = next_param_idx + 1;
        let offset_idx = next_param_idx + 2;
        let weight_idx = next_param_idx + 3;
        let half_life_idx = next_param_idx + 4;

        let recency_expr = format!(
            "(1.0 + ${w}::double precision * EXP(-EXTRACT(EPOCH FROM (CURRENT_TIMESTAMP - COALESCE(\
                CASE WHEN d.metadata->>'updated_at' IS NOT NULL \
                     AND pg_input_is_valid(d.metadata->>'updated_at', 'timestamptz') \
                THEN (d.metadata->>'updated_at')::timestamptz END, \
                d.updated_at))) / (86400.0 * ${h}::double precision)))::real",
            w = weight_idx,
            h = half_life_idx,
        );

        // `candidates` is MATERIALIZED so Postgres evaluates it once and so
        // the per-row `max_score` window agrees with the rows we filter on.
        // Without it, the CTE can be inlined and re-executed, and Tantivy's
        // TopN may iterate segments in a different order each time — producing
        // pages that don't agree across requests.
        let full_query = format!(
            r#"
            WITH candidates AS MATERIALIZED (
                SELECT id,
                       pdb.score(id) as bm25_score,
                       MAX(pdb.score(id)) OVER () as max_bm25_score
                FROM documents
                WHERE id @@@ pdb.parse($1, lenient => true){filter_where}
                ORDER BY bm25_score DESC, id
                LIMIT ${candidate_limit_idx}
            ),
            relevant AS (
                SELECT id, bm25_score
                FROM candidates
                WHERE bm25_score >= {min_score_ratio}::real * COALESCE(max_bm25_score, 0)
            ),
            ranked AS (
                SELECT r.id, (r.bm25_score * {recency_expr}) as score
                FROM relevant r
                JOIN documents d ON d.id = r.id
                ORDER BY score DESC, id
                LIMIT ${limit_idx} OFFSET ${offset_idx}
            )
            SELECT r.id, r.score,
                   d.source_id, d.external_id, d.title, d.content_id, d.content_type,
                   d.file_size, d.file_extension, d.url,
                   d.metadata, d.permissions, d.attributes, d.created_at, d.updated_at, d.last_indexed_at,
                   ARRAY[ts_headline('english', d.content,
                       plainto_tsquery('english', $2),
                       'StartSel=**, StopSel=**, MaxFragments=3, MaxWords=30, MinWords=10'
                   )] as content_snippets
            FROM ranked r
            JOIN documents d ON d.id = r.id
            ORDER BY r.score DESC, r.id"#,
            filter_where = filter_where,
            recency_expr = recency_expr,
            candidate_limit_idx = candidate_limit_idx,
            limit_idx = limit_idx,
            offset_idx = offset_idx,
            min_score_ratio = MIN_SCORE_RATIO,
        );
        debug!(
            sql = %full_query,
            tantivy_query = %tantivy_query,
            original_query = query,
            source_ids = ?source_ids,
            content_types = ?content_types,
            document_id = ?document_id,
            candidate_limit = CANDIDATE_LIMIT,
            limit = limit,
            offset = offset,
            recency_boost_weight = recency_boost_weight,
            recency_half_life_days = recency_half_life_days,
            "fulltext search query"
        );

        let mut query_builder = sqlx::query_as::<_, SearchHit>(&full_query)
            .bind(&tantivy_query)
            .bind(query);

        query_builder = query_builder.bind(source_ids);

        if let Some(ct) = content_types {
            if !ct.is_empty() {
                query_builder = query_builder.bind(ct);
            }
        }

        if let Some(doc_id) = document_id {
            query_builder = query_builder.bind(doc_id);
        }

        // The candidate cap is fixed (not page-derived) so that the relevance
        // threshold sees the same MAX(bm25_score) across pages of one query.
        query_builder = query_builder
            .bind(CANDIDATE_LIMIT)
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
        user_groups: &[String],
        date_filter: Option<&DateFilter>,
        person_filters: Option<&[String]>,
    ) -> Result<Vec<SearchHit>, DatabaseError> {
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
                    format!("metadata->>'author' ILIKE '%{escaped}%'")
                })
                .collect();
            if !conditions.is_empty() {
                filters.push(format!("({})", conditions.join(" OR ")));
            }
        }

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
            ORDER BY COALESCE(
                CASE WHEN metadata->>'updated_at' IS NOT NULL
                     AND pg_input_is_valid(metadata->>'updated_at', 'timestamptz')
                THEN (metadata->>'updated_at')::timestamptz END,
                updated_at) DESC
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

        // $1 = tantivy_query, $2+ = filter binds, then facet_limit.
        let Bm25Filters {
            tantivy_query,
            filter_where,
            next_param_idx,
        } = self
            .build_bm25_filters(
                query,
                source_ids,
                content_types,
                attribute_filters,
                user_email,
                user_groups,
                None, // facets do not narrow to a single document
                date_filter,
                person_filters,
                2,
            )
            .await?;

        let facet_limit_idx = next_param_idx;

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
            sqlx::query_as::<_, (String, String, i64)>(&query_str).bind(&tantivy_query);

        query_builder = query_builder.bind(source_ids);

        if let Some(ct) = content_types {
            if !ct.is_empty() {
                query_builder = query_builder.bind(ct);
            }
        }

        query_builder = query_builder.bind(CANDIDATE_LIMIT);

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
