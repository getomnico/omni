use serde_json::Value as JsonValue;
use shared::{
    db::error::DatabaseError,
    db::repositories::document,
    models::{AttributeFilter, DateFilter, Document, Facet, FacetValue},
};
use sqlx::{FromRow, PgPool};
use std::collections::{HashMap, HashSet};
use tracing::debug;

#[derive(FromRow)]
pub struct SearchHit {
    #[sqlx(flatten)]
    pub document: Document,
    pub score: f32,
    #[sqlx(default)]
    pub content_snippets: Option<Vec<String>>,
}

pub struct SearchDocumentRepository {
    pool: PgPool,
}

impl SearchDocumentRepository {
    pub fn new(pool: &PgPool) -> Self {
        Self { pool: pool.clone() }
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
                )
                .await;
        }

        // Tokenize query via ParadeDB: splits on non-alphanumeric, ASCII-folds.
        // No stemming or stopwords — dropping stopwords would remove valid words
        // in non-English languages (e.g. German "die", "in", "was").
        let raw_terms: Vec<String> = sqlx::query_scalar(
            "SELECT unnest($1::pdb.simple('ascii_folding=true')::text[])"
        )
        .bind(query)
        .fetch_all(&self.pool)
        .await?;

        let mut seen = HashSet::new();
        // Cap at 12 terms. Without stopword removal longer queries produce more
        // tokens than before. Each unique term adds 3 UNION ALL branches plus one
        // bind parameter, so this keeps query size and planner overhead bounded.
        let terms: Vec<String> = raw_terms
            .into_iter()
            .filter(|t| seen.insert(t.clone()))
            .take(12)
            .collect();

        // Bind params: $1 = full query, $2..$(1+N) = individual terms, then filters
        let mut param_idx = 2 + terms.len();

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

        // Per-term: best score across all tokenizer paths.
        // - title (simple): exact match on filenames/titles split by underscores etc.
        // - title_secondary (source_code): CamelCase splitting for code identifiers
        // - content (icu): Unicode word-boundary segmentation for any language
        // MAX(score) picks the best path per document — no language detection needed.
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

        // When the query tokenizes to zero terms (e.g. pure punctuation), fall
        // back to phrase scoring only.
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
        user_groups: &[String],
        date_filter: Option<&DateFilter>,
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

        // Tokenize query via ParadeDB — same pipeline as search()
        let raw_terms: Vec<String> = sqlx::query_scalar(
            "SELECT unnest($1::pdb.simple('ascii_folding=true')::text[])"
        )
        .bind(query)
        .fetch_all(&self.pool)
        .await?;

        let mut seen = HashSet::new();
        // Cap at 12 terms — same reasoning as search().
        let terms: Vec<String> = raw_terms
            .into_iter()
            .filter(|t| seen.insert(t.clone()))
            .take(12)
            .collect();

        // Bind params: $1 = full query, $2..$(1+N) = individual terms, then filters
        let mut param_idx = 2 + terms.len();

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

        let common_where = if filters.is_empty() {
            String::new()
        } else {
            format!(" AND {}", filters.join(" AND "))
        };

        // Per-term: best of title, title_secondary, content
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

        let phrase_branch = format!(
            "SELECT id, MAX(score) as score FROM (\
                SELECT id, pdb.score(id) as score FROM documents \
                WHERE title ### $1::pdb.slop(2)::pdb.boost(10){common_where} \
                UNION ALL \
                SELECT id, pdb.score(id) as score FROM documents \
                WHERE content ### $1::pdb.slop(2)::pdb.boost(5){common_where}\
            ) p GROUP BY id"
        );

        let query_str = if terms.is_empty() {
            // No terms after tokenization — phrase-only scoring
            format!(
                r#"
                WITH phrase_scores AS (
                    {phrase_branch}
                ),
                thresholded AS (
                    SELECT id FROM phrase_scores
                    WHERE score >= (SELECT MAX(score) FROM phrase_scores) * 0.15
                )
                SELECT 'source_type' as facet, s.source_type as value, count(*) as count
                FROM thresholded t
                JOIN documents d ON d.id = t.id
                JOIN sources s ON d.source_id = s.id
                GROUP BY s.source_type
                ORDER BY count DESC
                "#,
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
                scored AS (
                    SELECT c.id, (c.token_score + COALESCE(p.score, 0)) as score
                    FROM combined c
                    LEFT JOIN phrase_scores p ON c.id = p.id
                ),
                thresholded AS (
                    SELECT id FROM scored
                    WHERE score >= (SELECT MAX(score) FROM scored) * 0.15
                )
                SELECT 'source_type' as facet, s.source_type as value, count(*) as count
                FROM thresholded t
                JOIN documents d ON d.id = t.id
                JOIN sources s ON d.source_id = s.id
                GROUP BY s.source_type
                ORDER BY count DESC
                "#,
                term_union = term_branches.join("\nUNION ALL\n"),
                phrase_branch = phrase_branch,
            )
        };

        let mut query_builder = sqlx::query_as::<_, (String, String, i64)>(&query_str).bind(query);

        for term in &terms {
            query_builder = query_builder.bind(term.as_str());
        }

        query_builder = query_builder.bind(source_ids);

        if let Some(ct) = content_types {
            if !ct.is_empty() {
                query_builder = query_builder.bind(ct);
            }
        }

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
