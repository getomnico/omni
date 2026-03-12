use shared::{DatabasePool, DocumentRepository};
use std::collections::HashSet;
use std::sync::Arc;
use time::OffsetDateTime;
use tokio::sync::RwLock;
use tracing::{error, info};

struct PeopleCacheInner {
    people: HashSet<String>,
    last_indexed_at: Option<OffsetDateTime>,
}

#[derive(Clone)]
pub struct PeopleCache {
    data: Arc<RwLock<PeopleCacheInner>>,
    db_pool: Option<DatabasePool>,
}

impl PeopleCache {
    pub fn new(db_pool: DatabasePool) -> Self {
        Self {
            data: Arc::new(RwLock::new(PeopleCacheInner {
                people: HashSet::new(),
                last_indexed_at: None,
            })),
            db_pool: Some(db_pool),
        }
    }

    #[cfg(test)]
    pub fn with_people(people: HashSet<String>) -> Self {
        Self {
            data: Arc::new(RwLock::new(PeopleCacheInner {
                people,
                last_indexed_at: None,
            })),
            db_pool: None,
        }
    }

    pub async fn refresh(&self) -> anyhow::Result<()> {
        let db_pool = self
            .db_pool
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No database pool configured"))?;
        let repo = DocumentRepository::new(db_pool.pool());

        let max_ts = repo.fetch_max_last_indexed_at().await?;
        {
            let data = self.data.read().await;
            if data.last_indexed_at == max_ts && !data.people.is_empty() {
                return Ok(());
            }
        }

        let emails = repo.fetch_all_permission_users().await?;
        let mut people = HashSet::with_capacity(emails.len() * 2);

        for email in &emails {
            people.insert(email.clone());
            if let Some(local) = email.split('@').next() {
                for part in local.split(['.', '_', '-']) {
                    if part.len() >= 2 {
                        people.insert(part.to_lowercase());
                    }
                }
            }
        }

        let count = people.len();
        let mut data = self.data.write().await;
        data.people = people;
        data.last_indexed_at = max_ts;

        info!("People cache refreshed with {} entries", count);
        Ok(())
    }

    pub async fn contains(&self, term: &str) -> bool {
        let data = self.data.read().await;
        data.people.contains(&term.to_lowercase())
    }

    pub fn start_background_refresh(self: &Arc<Self>, interval_secs: u64) {
        let cache = Arc::clone(self);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(interval_secs));
            loop {
                interval.tick().await;
                if let Err(e) = cache.refresh().await {
                    error!("Failed to refresh people cache: {}", e);
                }
            }
        });
    }
}
