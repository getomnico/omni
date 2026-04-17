use serde_json::Value as JsonValue;
use shared::{db::error::DatabaseError, db::pool::UserConn, DatabasePool};
use sqlx::FromRow;
use std::collections::{HashMap, HashSet};
use tracing::debug;

/// Extra candidates fetched beyond offset+limit so that recency re-ranking
/// doesn't miss relevant results.
const CANDIDATE_PADDING: i64 = 200;

/// Maximum candidates considered for facet counts. TopN pushes this limit into
/// the Tantivy index scan, avoiding full result-set materialisation.
const FACET_CANDIDATE_LIMIT: i64 = 10_000;

#[derive(FromRow)]
pub struct SearchHit {
    #[sqlx(flatten)]
    pub document: shared::models::Document,
    pub score: f32,
    #[sqlx(default)]
    pub content_snippets: Option<Vec<String>>,
}

pub struct SearchDocumentRepository {
    pool: DatabasePool,
    user_id: Option<String>,
}

impl SearchDocumentRepository {
    pub fn new(pool: &DatabasePool, user_id: Option<String>) -> Self {
        Self {
            pool: pool.clone(),
            user_id,
        }
    }

    async fn acquire(&self) -> Result<UserConn<'_>, DatabaseError> {
        self.pool
            .acquire_user(&self.user_id.as_ref().map(|s| s.as_str()).unwrap_or(""))
            .await
    }

    pub async fn search(
        &self,
        query: &str,
        source_ids: &[String],
        content_types: Option<&[String]>,
        attribute_filters: Option<&HashMap<String, shared::models::AttributeFilter>>,
        limit: i64,
        offset: i64,
        document_id: Option<&str>,
        date_filter: Option<&shared::models::DateFilter>,
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
                    date_filter,
                    person_filters,
                )
                .await;
        }

        // Tokenize query via ParadeDB: splits on non-alphanumeric, ASCII-folds.
        // No stemming or stopwords — dropping stopwords would remove valid words
        // in non-English languages (e.g. German "die", "in", "was").
        let raw_terms: Vec<String> =
            sqlx::query_scalar("SELECT unnest($1::pdb.simple('ascii_folding=true')::text[])")
                .bind(query)
                .fetch_all(self.pool.pool())
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

        let tantivy_query = build_tantivy_query(&terms, query);

        // Bind params: $1 = tantivy query string, $2 = original query (for snippets), then filters
        let mut param_idx = 3;

        let mut filters = Vec::new();
        build_common_filters(
            &mut filters,
            &mut param_idx,
            source_ids,
            content_types,
            attribute_filters,
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

        let filter_where = if filters.is_empty() {
            String::new()
        } else {
            format!(" AND {}", filters.join(" AND "))
        };

        // Bind order: $1=tantivy_query, $2=original_query, filters...,
        // candidate_limit, limit, offset, recency_weight, recency_half_life
        let candidate_limit_idx = param_idx;
        let limit_idx = param_idx + 1;
        let offset_idx = param_idx + 2;
        let weight_idx = param_idx + 3;
        let half_life_idx = param_idx + 4;

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
            WITH candidates AS (
                SELECT id, pdb.score(id) as bm25_score
                FROM documents
                WHERE id @@@ pdb.parse($1, lenient => true){filter_where}
                ORDER BY bm25_score DESC
                LIMIT ${candidate_limit_idx}
            ),
            ranked AS (
                SELECT c.id, (c.bm25_score * {recency_expr}) as score
                FROM candidates c
                JOIN documents d ON d.id = c.id
                ORDER BY score DESC
                LIMIT ${limit_idx} OFFSET ${offset_idx}
            )
            SELECT r.id, r.score,
                   d.source_id, d.external_id, d.title, d.content_id, d.content_type,
                   d.file_size, d.file_extension, d.url,
                   d.metadata, d.permissions, d.attributes, d.created_at, d.updated_at, d.last_indexed_at,
                   ARRAY[ts_headline('english', d.content,
                       plainto_tsquery('english', $2),
                       'StartSel=**, StopSel=**, MaxFragments=3, MaxWords=10, MinWords=5'
                   )] as content_snippets
            FROM ranked r
            JOIN documents d ON d.id = r.id
            ORDER BY r.score DESC"#,
            filter_where = filter_where,
            recency_expr = recency_expr,
            candidate_limit_idx = candidate_limit_idx,
            limit_idx = limit_idx,
            offset_idx = offset_idx,
        );
        debug!("Full search query: {}", full_query);

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

        let candidate_limit = offset + limit + CANDIDATE_PADDING;
        query_builder = query_builder
            .bind(candidate_limit)
            .bind(limit)
            .bind(offset)
            .bind(recency_boost_weight as f64)
            .bind(recency_half_life_days as f64);

        let results = if self.user_id.is_some() {
            let mut conn = self.acquire().await?;
            query_builder.fetch_all(&mut *conn).await?
        } else {
            query_builder.fetch_all(self.pool.pool()).await?
        };

        Ok(results)
    }

    async fn filter_only_search(
        &self,
        source_ids: &[String],
        content_types: Option<&[String]>,
        attribute_filters: Option<&HashMap<String, shared::models::AttributeFilter>>,
        limit: i64,
        offset: i64,
        date_filter: Option<&shared::models::DateFilter>,
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

        let results = if self.user_id.is_some() {
            let mut conn = self.acquire().await?;
            query_builder.fetch_all(&mut *conn).await?
        } else {
            query_builder.fetch_all(self.pool.pool()).await?
        };
        Ok(results)
    }

    pub async fn get_facet_counts(
        &self,
        query: &str,
        source_ids: &[String],
        content_types: Option<&[String]>,
        attribute_filters: Option<&HashMap<String, shared::models::AttributeFilter>>,
        date_filter: Option<&shared::models::DateFilter>,
        person_filters: Option<&[String]>,
    ) -> Result<Vec<shared::models::Facet>, DatabaseError> {
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
            let rows = if self.user_id.is_some() {
                debug!("Executing empty-query facet with user conn");
                let mut conn = self.acquire().await?;
                qb.fetch_all(&mut *conn).await?
            } else {
                qb.fetch_all(self.pool.pool()).await?
            };
            return Ok(rows_to_facets(rows));
        }

        // Tokenize query via ParadeDB — same pipeline as search()
        let raw_terms: Vec<String> =
            sqlx::query_scalar("SELECT unnest($1::pdb.simple('ascii_folding=true')::text[])")
                .bind(query)
                .fetch_all(self.pool.pool())
                .await?;

        let mut seen = HashSet::new();
        // Cap at 12 terms — same reasoning as search().
        let terms: Vec<String> = raw_terms
            .into_iter()
            .filter(|t| seen.insert(t.clone()))
            .take(12)
            .collect();

        let tantivy_query = build_tantivy_query(&terms, query);

        // Bind params: $1 = tantivy query string, then filters
        let mut param_idx = 2;

        let mut filters = Vec::new();
        build_common_filters(
            &mut filters,
            &mut param_idx,
            source_ids,
            content_types,
            attribute_filters,
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
        debug!("Full facet query: {}", query_str);

        let mut query_builder =
            sqlx::query_as::<_, (String, String, i64)>(&query_str).bind(&tantivy_query);

        query_builder = query_builder.bind(source_ids);

        if let Some(ct) = content_types {
            if !ct.is_empty() {
                query_builder = query_builder.bind(ct);
            }
        }

        query_builder = query_builder.bind(FACET_CANDIDATE_LIMIT);

        let facet_rows = if self.user_id.is_some() {
            debug!(
                "Executing facet query with user conn for user_id: {:?}",
                self.user_id
            );
            let mut conn = self.acquire().await?;
            debug!("User conn acquired, executing facet query...");
            query_builder.fetch_all(&mut *conn).await
        } else {
            debug!("Executing facet query with pool conn");
            query_builder.fetch_all(self.pool.pool()).await
        };
        debug!("Facet query completed successfully, converting to facets");
        Ok(rows_to_facets(facet_rows?))
    }

    pub async fn get_distinct_attribute_values(
        &self,
        keys: &[String],
        limit: i64,
    ) -> Result<HashMap<String, Vec<String>>, DatabaseError> {
        if keys.is_empty() {
            return Ok(HashMap::new());
        }

        let query = sqlx::query_as::<_, (String, String)>(
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
        .bind(limit);

        let rows = if self.user_id.is_some() {
            let mut conn = self.acquire().await?;
            query.fetch_all(&mut *conn).await?
        } else {
            query.fetch_all(self.pool.pool()).await?
        };

        let mut result: HashMap<String, Vec<String>> = HashMap::new();
        for (key, val) in rows {
            result.entry(key).or_default().push(val);
        }
        Ok(result)
    }
}

fn rows_to_facets(rows: Vec<(String, String, i64)>) -> Vec<shared::models::Facet> {
    let mut facets_map: HashMap<String, Vec<shared::models::FacetValue>> = HashMap::new();
    for (facet_name, value, count) in rows {
        facets_map
            .entry(facet_name)
            .or_default()
            .push(shared::models::FacetValue {
                value,
                count: Some(count),
            });
    }
    facets_map
        .into_iter()
        .map(|(name, values)| shared::models::Facet { name, values })
        .collect()
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
    attribute_filters: Option<&HashMap<String, shared::models::AttributeFilter>>,
    date_filter: Option<&shared::models::DateFilter>,
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
                shared::models::AttributeFilter::Exact(value) => {
                    let term_value = json_value_to_term_string(value);
                    filters.push(format!(
                        "attributes @@@ '{}:{}'",
                        key.replace('\'', "''"),
                        term_value.replace('\'', "''")
                    ));
                }
                shared::models::AttributeFilter::AnyOf(values) => {
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
                shared::models::AttributeFilter::Range { gte, lte } => {
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
}
