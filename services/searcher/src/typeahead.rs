use fst::automaton::Subsequence;
use fst::{IntoStreamer, Map, MapBuilder, Streamer};
use shared::{DatabasePool, DocumentRepository};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{error, info};

use crate::models::TypeaheadResult;

pub struct TypeaheadEntry {
    pub document_id: String,
    pub title: String,
    pub url: Option<String>,
    pub source_id: String,
}

struct TitleData {
    fst: Map<Vec<u8>>,
    entries: Vec<TypeaheadEntry>,
}

impl TitleData {
    fn empty() -> Self {
        let builder = MapBuilder::memory();
        let fst = builder.into_map();
        Self {
            fst,
            entries: Vec::new(),
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
        let rows = repo.fetch_all_title_entries().await?;

        let mut entries: Vec<TypeaheadEntry> = Vec::with_capacity(rows.len());
        let mut keys: Vec<(String, u64)> = Vec::with_capacity(rows.len());

        for row in rows {
            let normalized = normalize(&row.title);
            if normalized.is_empty() {
                continue;
            }
            let idx = entries.len() as u64;
            entries.push(TypeaheadEntry {
                document_id: row.id,
                title: row.title,
                url: row.url,
                source_id: row.source_id,
            });
            keys.push((normalized, idx));
        }

        keys.sort_by(|a, b| a.0.cmp(&b.0));
        keys.dedup_by(|a, b| a.0 == b.0);

        let mut builder = MapBuilder::memory();
        for (key, idx) in &keys {
            builder.insert(key.as_bytes(), *idx)?;
        }
        let fst = builder.into_map();

        let new_data = TitleData { fst, entries };
        let mut data = self.data.write().await;
        *data = new_data;

        info!("Typeahead index refreshed with {} entries", keys.len());
        Ok(())
    }

    pub async fn search(&self, query: &str, limit: usize) -> Vec<TypeaheadResult> {
        let normalized = normalize(query);
        if normalized.is_empty() {
            return Vec::new();
        }

        let data = self.data.read().await;
        let automaton = Subsequence::new(&normalized);
        let mut stream = data.fst.search(automaton).into_stream();

        let mut results = Vec::with_capacity(limit);
        while let Some((_, idx)) = stream.next() {
            if let Some(entry) = data.entries.get(idx as usize) {
                results.push(TypeaheadResult {
                    document_id: entry.document_id.clone(),
                    title: entry.title.clone(),
                    url: entry.url.clone(),
                    source_id: entry.source_id.clone(),
                });
            }
            if results.len() >= limit {
                break;
            }
        }

        results
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
}
