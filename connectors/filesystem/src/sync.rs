use anyhow::{Context, Result};
use omni_connector_sdk::SyncContext;
use tracing::{info, warn};

use crate::models::FileSystemConfig;
use crate::scanner::FileSystemScanner;

pub async fn run_sync(
    source_name: String,
    source_config: FileSystemConfig,
    ctx: SyncContext,
) -> Result<()> {
    info!(
        "Starting filesystem sync for source: {} (sync_run_id: {})",
        ctx.source_id(),
        ctx.sync_run_id()
    );

    let scanner = FileSystemScanner::new(source_config.into_source(source_name));
    let files = scanner.scan_directory().await?;
    let total_scanned = files.len();
    let mut total_processed = 0usize;

    info!("Found {} files to process", total_scanned);

    for file in files {
        if ctx.is_cancelled() {
            info!("Sync cancelled, stopping scan");
            break;
        }

        let file_path = file.path.clone();
        let file_name = file.name.clone();
        let mime_type = file.mime_type.clone();

        let data = match std::fs::read(&file_path) {
            Ok(data) => data,
            Err(error) => {
                warn!("Failed to read file {}: {}", file_path.display(), error);
                continue;
            }
        };

        let content_id = match ctx
            .extract_and_store_content(data, &mime_type, Some(&file_name))
            .await
        {
            Ok(content_id) => content_id,
            Err(error) => {
                warn!(
                    "Failed to extract/store content for {}: {}",
                    file_path.display(),
                    error
                );
                continue;
            }
        };

        let event = file.to_connector_event(
            ctx.sync_run_id().to_string(),
            ctx.source_id().to_string(),
            content_id,
        );

        if let Err(error) = ctx.emit_event(event).await {
            warn!(
                "Failed to emit event for {}: {}",
                file_path.display(),
                error
            );
            continue;
        }

        total_processed += 1;

        if total_processed % 100 == 0 {
            info!("Processed {} files", total_processed);
            let _ = ctx.increment_scanned(100).await;
        }
    }

    if ctx.is_cancelled() {
        info!("Filesystem sync {} was cancelled", ctx.sync_run_id());
        ctx.cancel().await?;
        return Ok(());
    }

    ctx.complete(total_scanned as i32, total_processed as i32, None)
        .await
        .context("Failed to mark filesystem sync complete")?;

    Ok(())
}
