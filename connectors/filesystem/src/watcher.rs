//! Realtime filesystem watcher.
//!
//! Runs as a long-lived sync under `SyncType::Realtime`: the connector manager
//! triggers `/sync` once, the SDK invokes `FileSystemConnector::sync()`, and
//! this watcher loops emitting `ConnectorEvent`s via the `SyncContext` until
//! `ctx.is_cancelled()` flips. All event emission and lifecycle calls go
//! through the SDK — no direct database or queue access.

use crate::models::{FileSystemConfig, FileSystemSource};
use crate::scanner::FileSystemScanner;
use anyhow::Result;
use notify::{Config, Event, EventKind, RecursiveMode, Watcher};
use omni_connector_sdk::{ConnectorEvent, DocumentMetadata, DocumentPermissions, SyncContext};
use std::path::PathBuf;
use std::time::{Duration, SystemTime};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

#[derive(Debug, Clone)]
enum FsEvent {
    Created(PathBuf),
    Modified(PathBuf),
    Deleted(PathBuf),
}

pub async fn run_realtime(
    source_name: String,
    source_config: FileSystemConfig,
    ctx: SyncContext,
) -> Result<()> {
    let source = source_config.into_source(source_name);
    let scanner = FileSystemScanner::new(source.clone());

    let (tx, mut rx) = mpsc::unbounded_channel::<FsEvent>();

    // PollWatcher keeps its own background thread; the callback fires there.
    // Dropping the watcher at function exit stops polling.
    let source_for_cb = source.clone();
    let config = Config::default()
        .with_poll_interval(Duration::from_secs(2))
        .with_compare_contents(true);
    let mut watcher = notify::PollWatcher::new(
        move |result: notify::Result<Event>| match result {
            Ok(event) => {
                for fs_event in translate(&event, &source_for_cb) {
                    let _ = tx.send(fs_event);
                }
            }
            Err(error) => warn!("File watcher error: {}", error),
        },
        config,
    )?;
    watcher.watch(&source.base_path, RecursiveMode::Recursive)?;
    info!(
        "Realtime watcher running for source {} at {}",
        ctx.source_id(),
        source.base_path.display()
    );

    let mut scanned = 0i32;
    let mut updated = 0i32;

    // Heartbeats keep `last_activity_at` fresh so the CM's stale-sync sweeper
    // doesn't mark a quiet-but-healthy watcher as failed.
    let mut heartbeat_ticker = tokio::time::interval(Duration::from_secs(30));
    heartbeat_ticker.tick().await;

    while !ctx.is_cancelled() {
        tokio::select! {
            event = rx.recv() => match event {
                Some(event) => match handle_event(&ctx, &scanner, event).await {
                    Ok(Emitted::Yes) => {
                        scanned += 1;
                        updated += 1;
                    }
                    Ok(Emitted::Skipped) => {}
                    Err(error) => warn!("Failed to handle filesystem event: {}", error),
                },
                None => break,
            },
            _ = heartbeat_ticker.tick() => {
                if let Err(error) = ctx.heartbeat().await {
                    warn!("Heartbeat failed: {}", error);
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(500)) => {}
        }
    }

    drop(watcher);

    if ctx.is_cancelled() {
        info!("Realtime watcher cancelled for source {}", ctx.source_id());
        ctx.cancel().await?;
    } else {
        ctx.complete(scanned, updated, None).await?;
    }
    Ok(())
}

enum Emitted {
    Yes,
    Skipped,
}

fn translate(event: &Event, source: &FileSystemSource) -> Vec<FsEvent> {
    let mut out = Vec::new();
    for path in &event.paths {
        if !source.should_include_file(path) {
            continue;
        }
        if path.is_dir() {
            continue;
        }
        let fs_event = match event.kind {
            EventKind::Create(_) => FsEvent::Created(path.clone()),
            EventKind::Modify(_) => FsEvent::Modified(path.clone()),
            EventKind::Remove(_) => FsEvent::Deleted(path.clone()),
            _ => continue,
        };
        out.push(fs_event);
    }
    out
}

async fn handle_event(
    ctx: &SyncContext,
    scanner: &FileSystemScanner,
    event: FsEvent,
) -> Result<Emitted> {
    let (path, is_created) = match event {
        FsEvent::Deleted(path) => {
            let connector_event = ConnectorEvent::DocumentDeleted {
                sync_run_id: ctx.sync_run_id().to_string(),
                source_id: ctx.source_id().to_string(),
                document_id: path.to_string_lossy().to_string(),
            };
            ctx.emit_event(connector_event).await?;
            info!("Emitted delete event for {}", path.display());
            return Ok(Emitted::Yes);
        }
        FsEvent::Created(path) => (path, true),
        FsEvent::Modified(path) => (path, false),
    };

    let file = match scanner.get_file_info(&path).await? {
        Some(f) => f,
        None => {
            debug!("Skipping filtered file: {}", path.display());
            return Ok(Emitted::Skipped);
        }
    };

    let data = match std::fs::read(&file.path) {
        Ok(d) if !d.is_empty() => d,
        Ok(_) => {
            debug!("Skipping empty file: {}", path.display());
            return Ok(Emitted::Skipped);
        }
        Err(e) => {
            warn!("Failed to read {}: {}", file.path.display(), e);
            return Ok(Emitted::Skipped);
        }
    };

    let file_name = file.name.clone();
    let content_id = match ctx
        .extract_and_store_content(data, &file.mime_type, Some(&file_name))
        .await
    {
        Ok(id) => id,
        Err(e) => {
            warn!("Extract/store failed for {}: {}", file.path.display(), e);
            return Ok(Emitted::Skipped);
        }
    };

    let connector_event = if is_created {
        file.to_connector_event(
            ctx.sync_run_id().to_string(),
            ctx.source_id().to_string(),
            content_id,
        )
    } else {
        build_updated_event(&file, ctx, content_id)
    };
    ctx.emit_event(connector_event).await?;
    info!(
        "Emitted {} event for {}",
        if is_created { "create" } else { "update" },
        path.display()
    );
    Ok(Emitted::Yes)
}

fn build_updated_event(
    file: &crate::models::FileSystemFile,
    ctx: &SyncContext,
    content_id: String,
) -> ConnectorEvent {
    use time::OffsetDateTime;
    let to_offset = |t: Option<SystemTime>| {
        t.and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
            .and_then(|d| OffsetDateTime::from_unix_timestamp(d.as_secs() as i64).ok())
    };
    ConnectorEvent::DocumentUpdated {
        sync_run_id: ctx.sync_run_id().to_string(),
        source_id: ctx.source_id().to_string(),
        document_id: file.path.to_string_lossy().to_string(),
        content_id,
        metadata: DocumentMetadata {
            title: Some(file.name.clone()),
            author: None,
            created_at: to_offset(file.created_time),
            updated_at: to_offset(file.modified_time),
            content_type: None,
            mime_type: Some(file.mime_type.clone()),
            size: Some(file.size.to_string()),
            url: None,
            path: Some(file.path.to_string_lossy().to_string()),
            extra: None,
        },
        permissions: Some(DocumentPermissions {
            public: true,
            users: vec![],
            groups: vec![],
        }),
        attributes: None,
    }
}
