/// Confirmed content-type allowlist for @-mention candidates.
///
/// Normalized types:
///   document, spreadsheet, presentation, pdf, page
///
/// MIME/raw fallbacks for connectors that store MIME types instead of
/// normalized values:
///   text/plain, text/markdown, text/csv
///   application/pdf
///   application/vnd.openxmlformats-officedocument.wordprocessingml.document (docx)
///   application/msword (doc)
///   application/vnd.openxmlformats-officedocument.spreadsheetml.sheet (xlsx)
///   application/vnd.ms-excel (xls)
///   application/vnd.openxmlformats-officedocument.presentationml.presentation (pptx)
///   application/vnd.ms-powerpoint (ppt)
///   application/vnd.google-apps.document
///   application/vnd.google-apps.spreadsheet
///   application/vnd.google-apps.presentation
///
/// NULL and unknown values remain excluded (fail-closed).
pub const MENTIONABLE_CONTENT_TYPES: &[&str] = &[
    // Normalized
    "document",
    "spreadsheet",
    "presentation",
    "pdf",
    "page",
    // Text/file MIME fallbacks
    "text/plain",
    "text/markdown",
    "text/csv",
    // PDF explicit
    "application/pdf",
    // Microsoft Office fallbacks
    "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
    "application/msword",
    "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
    "application/vnd.ms-excel",
    "application/vnd.openxmlformats-officedocument.presentationml.presentation",
    "application/vnd.ms-powerpoint",
    // Google Workspace fallbacks
    "application/vnd.google-apps.document",
    "application/vnd.google-apps.spreadsheet",
    "application/vnd.google-apps.presentation",
];

use fst::automaton::Str;
use fst::{Automaton, IntoStreamer, Map, MapBuilder, Streamer};
use shared::{DatabasePool, DocumentRepository};
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{error, info};

use crate::models::TypeaheadResult;

/// A scored candidate entry returned by the FST before ACL filtering.
/// The handler filters by permission and then applies the final limit.
#[derive(Debug, Clone)]
pub struct ScoredCandidate {
    pub score: i64,
    pub document_id: String,
    pub title: String,
    pub url: Option<String>,
    pub source_id: String,
    pub source_type: String,
    pub content_type: String,
}

pub struct TypeaheadEntry {
    pub document_id: String,
    pub title: String,
    pub url: Option<String>,
    pub source_id: String,
    pub source_type: String,
    pub content_type: String,
}

struct TitleData {
    fst: Map<Vec<u8>>,
    entries: Vec<TypeaheadEntry>,
    normalized_titles: Vec<String>,
}

impl TitleData {
    fn empty() -> Self {
        let builder = MapBuilder::memory();
        let fst = builder.into_map();
        Self {
            fst,
            entries: Vec::new(),
            normalized_titles: Vec::new(),
        }
    }
}

#[derive(Clone)]
pub struct TitleIndex {
    data: Arc<RwLock<TitleData>>,
    db_pool: DatabasePool,
}

impl TitleIndex {
    pub fn new(db_pool: DatabasePool) -> Self {
        Self {
            data: Arc::new(RwLock::new(TitleData::empty())),
            db_pool,
        }
    }

    pub async fn refresh(&self) -> anyhow::Result<()> {
        let repo = DocumentRepository::new(self.db_pool.pool());
        let types: Vec<String> = MENTIONABLE_CONTENT_TYPES
            .iter()
            .map(|s| s.to_string())
            .collect();
        let rows = repo.fetch_title_entries_by_types(&types).await?;

        let mut entries: Vec<TypeaheadEntry> = Vec::with_capacity(rows.len());
        let mut normalized_titles: Vec<String> = Vec::with_capacity(rows.len());
        let mut keys: Vec<(Vec<u8>, u64)> = Vec::new();

        for row in rows {
            let Some(content_type) = row.content_type else {
                continue;
            };
            let normalized = normalize(&row.title);
            if normalized.is_empty() {
                continue;
            }
            let idx = entries.len() as u32;
            entries.push(TypeaheadEntry {
                document_id: row.id,
                title: row.title,
                url: row.url,
                source_id: row.source_id,
                source_type: row.source_type,
                content_type,
            });
            normalized_titles.push(normalized.clone());

            for word_start in std::iter::once(0).chain(
                normalized
                    .char_indices()
                    .filter(|(_, c)| *c == ' ')
                    .map(|(i, _)| i + 1),
            ) {
                let suffix = &normalized[word_start..];
                let mut key = Vec::with_capacity(suffix.len() + 1 + 4);
                key.extend_from_slice(suffix.as_bytes());
                key.push(0x00);
                key.extend_from_slice(&idx.to_be_bytes());
                keys.push((key, idx as u64));
            }
        }

        keys.sort_by(|a, b| a.0.cmp(&b.0));

        let mut builder = MapBuilder::memory();
        for (key, idx) in &keys {
            builder.insert(key, *idx)?;
        }
        let fst = builder.into_map();

        let new_data = TitleData {
            fst,
            entries,
            normalized_titles,
        };
        let mut data = self.data.write().await;
        *data = new_data;

        info!("Typeahead index refreshed with {} entries", keys.len());
        Ok(())
    }

    /// Return the full set of score-ordered candidate entries for a query.
    /// No cap is applied — the caller (handler) processes batches for ACL
    /// filtering and then applies the user-requested limit.
    pub async fn search_candidates(&self, query: &str) -> Vec<ScoredCandidate> {
        let normalized = normalize(query);
        if normalized.is_empty() {
            return Vec::new();
        }

        let data = self.data.read().await;
        let automaton = Str::new(&normalized).starts_with();
        let mut stream = data.fst.search(automaton).into_stream();

        let mut seen = HashSet::new();
        let mut candidates: Vec<(i64, usize)> = Vec::new();
        while let Some((_key_bytes, idx)) = stream.next() {
            let idx = idx as usize;
            if !seen.insert(idx) {
                continue;
            }
            if let Some(full_title) = data.normalized_titles.get(idx) {
                let score = score_match(&normalized, full_title);
                candidates.push((score, idx));
            }
        }

        candidates.sort_by(|a, b| b.0.cmp(&a.0));

        candidates
            .iter()
            .filter_map(|(score, idx)| {
                data.entries.get(*idx).map(|entry| ScoredCandidate {
                    score: *score,
                    document_id: entry.document_id.clone(),
                    title: entry.title.clone(),
                    url: entry.url.clone(),
                    source_id: entry.source_id.clone(),
                    source_type: entry.source_type.clone(),
                    content_type: entry.content_type.clone(),
                })
            })
            .collect()
    }

    /// Legacy convenience: apply ACL-unfiltered search with a fixed limit.
    /// Used by existing callers and tests that do not need permission checks.
    pub async fn search(&self, query: &str, limit: usize) -> Vec<TypeaheadResult> {
        self.search_candidates(query)
            .await
            .into_iter()
            .take(limit)
            .map(|c| TypeaheadResult {
                document_id: c.document_id,
                title: c.title,
                url: c.url,
                source_id: c.source_id,
                source_type: c.source_type,
                content_type: c.content_type,
            })
            .collect()
    }

    pub fn start_background_refresh(self: &Arc<Self>, interval_secs: u64) {
        let index = Arc::clone(self);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(interval_secs));
            loop {
                interval.tick().await;
                if let Err(e) = index.refresh().await {
                    error!("Failed to refresh typeahead index: {}", e);
                }
            }
        });
    }
}

pub fn normalize(title: &str) -> String {
    let lowered = title.to_lowercase();
    let replaced: String = lowered
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect();
    replaced.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn score_match(query: &str, title: &str) -> i64 {
    if let Some(score) = score_word_prefix_match(query, title) {
        return score;
    }
    score_character_alignment(query, title)
}

fn score_word_prefix_match(query: &str, title: &str) -> Option<i64> {
    let query_words: Vec<&str> = query.split_whitespace().collect();
    let title_words: Vec<&str> = title.split_whitespace().collect();

    if query_words.is_empty() || title_words.is_empty() {
        return None;
    }

    let assignments = find_word_assignments(&query_words, &title_words, 0, 0)?;

    let mut score: i64 = 10_000;

    if assignments[0] == 0 {
        score += 2_000;
    }

    for window in assignments.windows(2) {
        if window[1] == window[0] + 1 {
            score += 2500;
        }
    }

    for (qi, &ti) in assignments.iter().enumerate() {
        let qw = query_words[qi];
        let tw = title_words[ti];
        if qw == tw {
            score += 800;
        } else {
            score += (800 * qw.len() as i64) / tw.len() as i64;
        }
    }

    score -= title.len() as i64;

    Some(score)
}

fn find_word_assignments(
    query_words: &[&str],
    title_words: &[&str],
    qi: usize,
    min_ti: usize,
) -> Option<Vec<usize>> {
    if qi >= query_words.len() {
        return Some(Vec::new());
    }

    for ti in min_ti..title_words.len() {
        if title_words[ti].starts_with(query_words[qi]) {
            if let Some(mut rest) = find_word_assignments(query_words, title_words, qi + 1, ti + 1)
            {
                rest.insert(0, ti);
                return Some(rest);
            }
        }
    }

    None
}

fn score_character_alignment(query: &str, title: &str) -> i64 {
    let query_chars: Vec<char> = query.chars().collect();
    let title_chars: Vec<char> = title.chars().collect();

    if query_chars.is_empty() {
        return -(title.len() as i64);
    }

    let mut best_score = i64::MIN;

    for start in 0..title_chars.len() {
        if title_chars[start] == query_chars[0] {
            if let Some(s) = evaluate_alignment_from(&query_chars, &title_chars, start) {
                if s > best_score {
                    best_score = s;
                }
            }
        }
    }

    if best_score == i64::MIN {
        best_score = -(title.len() as i64) - 1000;
    }

    best_score - title.len() as i64
}

fn evaluate_alignment_from(
    query_chars: &[char],
    title_chars: &[char],
    start: usize,
) -> Option<i64> {
    let title_word_boundaries: Vec<bool> = title_chars
        .iter()
        .enumerate()
        .map(|(i, _)| i == 0 || title_chars[i - 1] == ' ')
        .collect();

    let mut score: i64 = 0;
    let mut qi = 0;
    let mut ti = start;
    let mut consecutive = 0;
    let mut last_match_ti: Option<usize> = None;

    if start == 0 {
        score += 50;
    }

    while qi < query_chars.len() && ti < title_chars.len() {
        if title_chars[ti] == query_chars[qi] {
            consecutive += 1;
            score += 10 + consecutive.min(5);

            if title_word_boundaries[ti] {
                score += 20;
            }

            if let Some(last) = last_match_ti {
                let gap = ti - last - 1;
                score -= 3 * gap as i64;
            }

            last_match_ti = Some(ti);
            qi += 1;
            ti += 1;
        } else {
            consecutive = 0;
            ti += 1;
        }
    }

    if qi == query_chars.len() {
        Some(score)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_basic() {
        assert_eq!(normalize("Budget Q4 2024"), "budget q4 2024");
    }

    #[test]
    fn test_normalize_special_chars() {
        assert_eq!(normalize("Budget (Q4-2024).xlsx"), "budget q4 2024 xlsx");
    }

    #[test]
    fn test_normalize_consecutive_spaces() {
        assert_eq!(normalize("  hello   world  "), "hello world");
    }

    #[test]
    fn test_normalize_empty() {
        assert_eq!(normalize(""), "");
        assert_eq!(normalize("---"), "");
    }

    #[test]
    fn test_score_word_prefix_beats_character_level() {
        let score = score_match("budg", "budget q4 2024");
        assert!(
            score >= 10_000,
            "word-prefix match should score >= 10,000, got {score}"
        );
    }

    #[test]
    fn test_score_title_start_bonus() {
        let score_start = score_match("budg", "budget q4 2024");
        let score_mid = score_match("budg", "q4 budget 2024");
        assert!(
            score_start > score_mid,
            "title-start match ({score_start}) should beat mid-title ({score_mid})"
        );
    }

    #[test]
    fn test_score_consecutive_words_bonus() {
        let score_consec = score_match("lion king", "the lion king");
        let score_split = score_match("lion king", "lion of the king");
        assert!(
            score_consec > score_split,
            "consecutive words ({score_consec}) should beat split ({score_split})"
        );
    }

    #[test]
    fn test_score_exact_word_beats_prefix() {
        let score_exact = score_match("budget", "budget q4 2024");
        let score_prefix = score_match("budg", "budget q4 2024");
        assert!(
            score_exact > score_prefix,
            "exact word ({score_exact}) should beat prefix ({score_prefix})"
        );
    }

    #[test]
    fn test_score_shorter_title_preferred() {
        let score_short = score_match("budget", "budget 2024");
        let score_long = score_match("budget", "budget for the entire fiscal year 2024");
        assert!(
            score_short > score_long,
            "shorter title ({score_short}) should beat longer ({score_long})"
        );
    }

    #[test]
    fn test_score_character_fallback() {
        let score = score_match("bgt", "budget");
        assert!(
            score < 10_000,
            "character-level match should score < 10,000, got {score}"
        );
    }

    #[test]
    fn test_score_multi_word_prefix() {
        let score = score_match("lion king", "list of the lion king characters");
        assert!(
            score >= 10_000,
            "multi-word prefix match should score >= 10,000, got {score}"
        );
    }

    #[test]
    fn test_mentionable_content_types_cover_confirmed_allowlist() {
        let allowed: &[&str] = MENTIONABLE_CONTENT_TYPES;
        // Normalized types
        assert!(allowed.contains(&"document"));
        assert!(allowed.contains(&"spreadsheet"));
        assert!(allowed.contains(&"presentation"));
        assert!(allowed.contains(&"pdf"));
        assert!(allowed.contains(&"page"));
        // Text/file MIME fallbacks
        assert!(allowed.contains(&"text/plain"));
        assert!(allowed.contains(&"text/markdown"));
        assert!(allowed.contains(&"text/csv"));
        // PDF explicit
        assert!(allowed.contains(&"application/pdf"));
        // Microsoft Office
        assert!(allowed
            .contains(&"application/vnd.openxmlformats-officedocument.wordprocessingml.document"));
        assert!(allowed.contains(&"application/msword"));
        assert!(
            allowed.contains(&"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet")
        );
        assert!(allowed.contains(&"application/vnd.ms-excel"));
        assert!(allowed.contains(
            &"application/vnd.openxmlformats-officedocument.presentationml.presentation"
        ));
        assert!(allowed.contains(&"application/vnd.ms-powerpoint"));
        // Google Workspace
        assert!(allowed.contains(&"application/vnd.google-apps.document"));
        assert!(allowed.contains(&"application/vnd.google-apps.spreadsheet"));
        assert!(allowed.contains(&"application/vnd.google-apps.presentation"));
        // Count check
        assert_eq!(allowed.len(), 18);
    }

    #[test]
    fn test_unknown_and_null_types_not_in_allowlist() {
        let allowed: &[&str] = MENTIONABLE_CONTENT_TYPES;
        assert!(!allowed.contains(&"email"));
        assert!(!allowed.contains(&"email_thread"));
        assert!(!allowed.contains(&"contact"));
        assert!(!allowed.contains(&"message"));
        assert!(!allowed.contains(&"chat"));
        assert!(!allowed.contains(&"event"));
        assert!(!allowed.contains(&"issue"));
        assert!(!allowed.contains(&"employee_profile"));
        assert!(!allowed.contains(&"meeting_transcript"));
        assert!(!allowed.contains(&"webpage"));
    }
}
