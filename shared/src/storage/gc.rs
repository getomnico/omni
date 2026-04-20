use super::{ObjectStorage, StorageError};
use crate::db::repositories::{ContentBlobRepository, OrphanStats};
use anyhow::Result;
use serde::Serialize;
use sqlx::PgPool;
use std::sync::Arc;
use tracing::{debug, info, warn};

/// Configuration for garbage collection
#[derive(Debug, Clone)]
pub struct GCConfig {
    /// Retention period in days before orphaned blobs are deleted
    pub retention_days: i32,
    /// Batch size for processing orphans
    pub batch_size: i32,
    /// Whether to run in dry-run mode (identify but don't delete)
    pub dry_run: bool,
}

impl Default for GCConfig {
    fn default() -> Self {
        Self {
            retention_days: 7,
            batch_size: 100,
            dry_run: false,
        }
    }
}

impl GCConfig {
    pub fn from_env() -> Self {
        let retention_days = std::env::var("GC_RETENTION_DAYS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(7);

        let batch_size = std::env::var("GC_BATCH_SIZE")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(100);

        let dry_run = std::env::var("GC_DRY_RUN")
            .ok()
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false);

        Self {
            retention_days,
            batch_size,
            dry_run,
        }
    }
}

/// Result of a GC run
#[derive(Debug, Default, Serialize)]
pub struct GCResult {
    /// Number of new orphans marked in this run
    pub orphans_marked: i64,
    /// Number of blobs unmarked (no longer orphaned)
    pub orphans_unmarked: i64,
    /// Number of blobs deleted
    pub blobs_deleted: i64,
    /// Total bytes reclaimed
    pub bytes_reclaimed: i64,
    /// Errors encountered during deletion
    pub errors: Vec<String>,
}

/// Content blob garbage collector
pub struct ContentBlobGC {
    repo: ContentBlobRepository,
    storage: Arc<dyn ObjectStorage>,
    config: GCConfig,
}

impl ContentBlobGC {
    pub fn new(pool: PgPool, storage: Arc<dyn ObjectStorage>, config: GCConfig) -> Self {
        Self {
            repo: ContentBlobRepository::new(&pool),
            storage,
            config,
        }
    }

    /// Run full GC cycle
    pub async fn run(&self) -> Result<GCResult> {
        info!("Starting content blob garbage collection");
        info!(
            "Config: retention_days={}, batch_size={}, dry_run={}",
            self.config.retention_days, self.config.batch_size, self.config.dry_run
        );

        // Phase 1: Mark new orphans
        let marked = self.repo.mark_orphans().await?;
        if marked > 0 {
            info!("Marked {} content blobs as orphaned", marked);
        }

        // Phase 2: Unmark blobs that got re-referenced
        let unmarked = self.repo.unmark_non_orphans().await?;
        if unmarked > 0 {
            info!("Unmarked {} content blobs (no longer orphaned)", unmarked);
        }

        // Phase 3: Delete expired orphans
        let mut result = self.delete_expired_orphans().await?;
        result.orphans_marked = marked;
        result.orphans_unmarked = unmarked;

        info!(
            "GC completed: marked={}, unmarked={}, deleted={}, bytes_reclaimed={}, errors={}",
            result.orphans_marked,
            result.orphans_unmarked,
            result.blobs_deleted,
            result.bytes_reclaimed,
            result.errors.len()
        );

        Ok(result)
    }

    async fn delete_expired_orphans(&self) -> Result<GCResult> {
        // Hard cap per invocation so a large backlog cannot monopolize the GC
        // task for hours and delay other maintenance work. Leftover orphans are
        // picked up on the next scheduled run.
        const MAX_BLOBS_PER_RUN: i64 = 50_000;

        let mut result = GCResult::default();
        let mut processed_total: i64 = 0;

        while processed_total < MAX_BLOBS_PER_RUN {
            let remaining = MAX_BLOBS_PER_RUN - processed_total;
            let batch_size = (self.config.batch_size as i64).min(remaining) as i32;

            let orphans = self
                .repo
                .fetch_expired_orphans(self.config.retention_days, batch_size)
                .await?;

            if orphans.is_empty() {
                break;
            }

            let batch_count = orphans.len();
            processed_total += batch_count as i64;

            if self.config.dry_run {
                let total_size: i64 = orphans.iter().map(|o| o.size_bytes).sum();
                info!(
                    "[DRY RUN] Would delete {} expired orphan blobs ({} bytes)",
                    batch_count, total_size
                );
                break;
            }

            for orphan in &orphans {
                match self.storage.delete_content(&orphan.id).await {
                    Ok(_) => {
                        result.blobs_deleted += 1;
                        result.bytes_reclaimed += orphan.size_bytes;
                        debug!("Deleted orphan blob: id={}", orphan.id);
                    }
                    Err(StorageError::NotFound(_)) => {
                        // Already deleted, count as success
                        result.blobs_deleted += 1;
                        debug!("Orphan blob already deleted: id={}", orphan.id);
                    }
                    Err(e) => {
                        let error_msg = format!("Failed to delete blob {}: {}", orphan.id, e);
                        warn!("{}", error_msg);
                        result.errors.push(error_msg);
                    }
                }
            }

            info!("Processed batch of {} expired orphans", batch_count);
        }

        Ok(result)
    }

    /// Get current orphan statistics
    pub async fn get_orphan_stats(&self) -> Result<OrphanStats> {
        Ok(self
            .repo
            .get_orphan_stats(self.config.retention_days)
            .await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gc_config_defaults() {
        let config = GCConfig::default();
        assert_eq!(config.retention_days, 7);
        assert_eq!(config.batch_size, 100);
        assert!(!config.dry_run);
    }
}
