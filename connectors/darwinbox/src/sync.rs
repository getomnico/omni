use std::collections::BTreeSet;

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Duration, Utc};
use omni_connector_sdk::{ConnectorEvent, DocumentMetadata, SyncContext, SyncType};
use serde_json::{json, Value as JsonValue};
use tracing::{info, warn};

use crate::client::DarwinboxClient;
use crate::config::{self, DarwinboxSourceConfig};
use crate::mappers;
use crate::models::{DarwinboxCheckpoint, DarwinboxSyncModuleKey, ModuleCheckpoint};

const INCREMENTAL_OVERLAP_SECONDS: i64 = 900;

pub async fn run_sync(
    client: &DarwinboxClient,
    config: &DarwinboxSourceConfig,
    state: Option<DarwinboxCheckpoint>,
    ctx: SyncContext,
) -> Result<()> {
    info!(
        source_id = ctx.source_id(),
        sync_run_id = ctx.sync_run_id(),
        "Starting Darwinbox sync"
    );

    let mut checkpoint = state.unwrap_or_default();
    if checkpoint.schema_version != 0 && checkpoint.schema_version < 2 {
        bail!("legacy Darwinbox checkpoint is incompatible; delete and recreate the source");
    }
    let policy_fingerprint = serde_json::to_string(config)?;
    if checkpoint.policy_fingerprint.as_deref() != Some(policy_fingerprint.as_str()) {
        for document_id in checkpoint.indexed_document_ids.clone() {
            ctx.emit_event(ConnectorEvent::DocumentDeleted {
                sync_run_id: ctx.sync_run_id().to_string(),
                source_id: ctx.source_id().to_string(),
                document_id,
            })
            .await?;
        }
        checkpoint.schema_version = 2;
        checkpoint.policy_fingerprint = Some(policy_fingerprint.clone());
        checkpoint.indexed_document_ids.clear();
        ctx.save_checkpoint(json!(checkpoint)).await?;
    }
    let previous_ids = checkpoint.indexed_document_ids.clone();
    let mut current_ids = BTreeSet::new();
    // Intermediate checkpoints retain every previously durable document plus
    // newly emitted documents. If a later operation fails, the next run still
    // has a complete revocation inventory.
    let mut durable_ids = previous_ids.clone();
    checkpoint.schema_version = 2;

    // Employee directory — always processed if enabled, uses config for scope/ACL
    if config.sync_modules.employee_directory {
        sync_employee_directory(
            client,
            config,
            ctx.source_id(),
            &ctx,
            &mut checkpoint,
            &policy_fingerprint,
            &mut current_ids,
            &mut durable_ids,
        )
        .await?;
        set_module_watermark(
            &mut checkpoint,
            DarwinboxSyncModuleKey::EmployeeDirectory,
            Utc::now().to_rfc3339(),
        );
        save_inventory_checkpoint(&ctx, &mut checkpoint, &policy_fingerprint, &durable_ids).await?;
    }

    // Deleted employees
    if config.sync_modules.deleted_employees {
        let since = module_since(&checkpoint, DarwinboxSyncModuleKey::DeletedEmployees, &ctx);
        sync_deleted_employees(client, since.as_deref(), &ctx).await?;
        set_module_watermark(
            &mut checkpoint,
            DarwinboxSyncModuleKey::DeletedEmployees,
            Utc::now().to_rfc3339(),
        );
        save_inventory_checkpoint(&ctx, &mut checkpoint, &policy_fingerprint, &durable_ids).await?;
    }

    // Org masters — each entity type as its own flag
    // Departments
    if config.sync_modules.departments {
        sync_single_org_master(
            client,
            config,
            "department",
            "darwinbox:department",
            "/orgmasterapi/departmentlist",
            &ctx,
            &mut current_ids,
        )
        .await?;
    }
    // Designations
    if config.sync_modules.designations {
        sync_single_org_master(
            client,
            config,
            "designation",
            "darwinbox:designation",
            "/orgmasterapi/designationlist",
            &ctx,
            &mut current_ids,
        )
        .await?;
    }
    // Office locations
    if config.sync_modules.office_locations {
        sync_single_org_master(
            client,
            config,
            "office_location",
            "darwinbox:office_location",
            "/orgmasterapi/officelocationlist",
            &ctx,
            &mut current_ids,
        )
        .await?;
    }
    // Business units
    if config.sync_modules.business_units {
        sync_single_org_master(
            client,
            config,
            "business_unit",
            "darwinbox:business_unit",
            "/orgmasterapi/businessunitlist",
            &ctx,
            &mut current_ids,
        )
        .await?;
    }
    // Divisions
    if config.sync_modules.divisions {
        sync_single_org_master(
            client,
            config,
            "division",
            "darwinbox:division",
            "/orgmasterapi/divisionlist",
            &ctx,
            &mut current_ids,
        )
        .await?;
    }
    // Cost centers
    if config.sync_modules.cost_centers {
        sync_single_org_master(
            client,
            config,
            "cost_center",
            "darwinbox:cost_center",
            "/orgmasterapi/costcenterlist",
            &ctx,
            &mut current_ids,
        )
        .await?;
    }
    // Group companies
    if config.sync_modules.group_companies {
        sync_single_org_master(
            client,
            config,
            "group_company",
            "darwinbox:group_company",
            "/orgmasterapi/groupcompanylist",
            &ctx,
            &mut current_ids,
        )
        .await?;
    }

    // Positions
    if config.sync_modules.positions {
        let response = client.fetch_position_master(None).await?;
        sync_typed_collection(
            client,
            config,
            "position",
            "darwinbox:position",
            &response,
            ctx.source_id(),
            &ctx,
            mappers::format_position_item,
            &mut current_ids,
        )
        .await?;
        set_module_watermark(
            &mut checkpoint,
            DarwinboxSyncModuleKey::PositionMaster,
            Utc::now().to_rfc3339(),
        );
        save_inventory_checkpoint(&ctx, &mut checkpoint, &policy_fingerprint, &current_ids).await?;
    }

    // Holidays
    if config.sync_modules.holidays {
        sync_holidays(client, config, ctx.source_id(), &ctx, &mut current_ids).await?;
        set_module_watermark(
            &mut checkpoint,
            DarwinboxSyncModuleKey::Holidays,
            Utc::now().to_rfc3339(),
        );
        save_inventory_checkpoint(&ctx, &mut checkpoint, &policy_fingerprint, &current_ids).await?;
    }

    // ATS jobs
    if config.sync_modules.ats_jobs {
        let response = client.fetch_jobs(None).await?;
        sync_typed_collection(
            client,
            config,
            "ats_job",
            "darwinbox:job",
            &response,
            ctx.source_id(),
            &ctx,
            mappers::format_ats_job_item,
            &mut current_ids,
        )
        .await?;
        set_module_watermark(
            &mut checkpoint,
            DarwinboxSyncModuleKey::AtsJobs,
            Utc::now().to_rfc3339(),
        );
        save_inventory_checkpoint(&ctx, &mut checkpoint, &policy_fingerprint, &current_ids).await?;
    }

    for document_id in previous_ids.difference(&current_ids) {
        ctx.emit_event(ConnectorEvent::DocumentDeleted {
            sync_run_id: ctx.sync_run_id().to_string(),
            source_id: ctx.source_id().to_string(),
            document_id: document_id.clone(),
        })
        .await?;
    }
    save_inventory_checkpoint(&ctx, &mut checkpoint, &policy_fingerprint, &current_ids).await?;

    info!(source_id = ctx.source_id(), "Darwinbox sync completed");
    Ok(())
}

async fn sync_employee_directory(
    client: &DarwinboxClient,
    config: &DarwinboxSourceConfig,
    source_id: &str,
    ctx: &SyncContext,
    checkpoint: &mut DarwinboxCheckpoint,
    policy_fingerprint: &str,
    current_ids: &mut BTreeSet<String>,
    durable_ids: &mut BTreeSet<String>,
) -> Result<()> {
    // Always fetch a complete snapshot so policy reconciliation is authoritative.
    let response = client
        .fetch_employees(None, None)
        .await
        .context("failed to fetch Darwinbox employee directory")?;

    let mut emitted = 0i32;

    for employee in response.employee_data {
        if ctx.is_cancelled() {
            ctx.cancel().await?;
            return Ok(());
        }

        let Some(document_id) = employee.external_id() else {
            warn!("Skipping Darwinbox employee without employee_id");
            continue;
        };

        // Apply employee scope filter from config
        if !config.is_employee_in_scope(&employee) {
            continue;
        }

        // Use filtered content with only approved fields
        let content = employee.content_filtered(&config.employee_fields);
        let content_id = ctx
            .store_content(&content)
            .await
            .with_context(|| format!("failed to store content for {document_id}"))?;

        // Build event with config-derived non-public permissions
        let permissions = config::document_permissions(
            "employee_profile",
            config,
            source_id,
            employee.company_email_id.as_deref(),
        );

        if let Some(event) = employee.to_event_with_permissions(
            ctx.sync_run_id().to_string(),
            source_id.to_string(),
            content_id,
            content.len(),
            &config.employee_fields,
            permissions,
        ) {
            // Persist a conservatively over-inclusive revocation inventory before
            // the durable event can become visible to the indexer.
            durable_ids.insert(document_id.clone());
            save_inventory_checkpoint(ctx, checkpoint, policy_fingerprint, durable_ids).await?;
            ctx.emit_event(event).await?;
            current_ids.insert(document_id);
            emitted += 1;
        }

        if emitted > 0 && emitted % 100 == 0 {
            ctx.increment_scanned(100).await?;
        }
    }

    if emitted % 100 != 0 {
        ctx.increment_scanned(emitted % 100).await?;
    }

    Ok(())
}

async fn sync_deleted_employees(
    client: &DarwinboxClient,
    last_modified: Option<&str>,
    ctx: &SyncContext,
) -> Result<()> {
    let provider_last_modified = last_modified.and_then(to_darwinbox_timestamp_with_overlap);
    let response = client
        .fetch_deleted_employees(provider_last_modified.as_deref())
        .await
        .context("failed to fetch deleted Darwinbox employees")?;

    let cols = response
        .get("cols")
        .and_then(JsonValue::as_array)
        .map(|cols| {
            cols.iter()
                .filter_map(JsonValue::as_str)
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let employee_id_idx = cols
        .iter()
        .position(|col| matches!(col.as_str(), "Candidate ID" | "Employee ID" | "Employee No"));

    let Some(employee_id_idx) = employee_id_idx else {
        warn!("Deleted employees response did not include an employee ID column");
        return Ok(());
    };

    let rows = response
        .get("output")
        .and_then(JsonValue::as_array)
        .cloned()
        .unwrap_or_default();

    let mut deleted = 0i32;
    for row in rows {
        if ctx.is_cancelled() {
            ctx.cancel().await?;
            return Ok(());
        }
        let Some(employee_id) = row
            .as_array()
            .and_then(|values| values.get(employee_id_idx))
            .and_then(JsonValue::as_str)
            .filter(|id| !id.trim().is_empty())
        else {
            continue;
        };
        ctx.emit_event(ConnectorEvent::DocumentDeleted {
            sync_run_id: ctx.sync_run_id().to_string(),
            source_id: ctx.source_id().to_string(),
            document_id: format!("darwinbox:employee:{employee_id}"),
        })
        .await?;
        deleted += 1;
    }

    if deleted > 0 {
        ctx.increment_scanned(deleted).await?;
    }

    Ok(())
}

/// Sync a single org master entity type using its typed mapper.
async fn sync_single_org_master(
    client: &DarwinboxClient,
    config: &DarwinboxSourceConfig,
    content_type: &str,
    external_prefix: &str,
    path: &str,
    ctx: &SyncContext,
    current_ids: &mut BTreeSet<String>,
) -> Result<()> {
    if ctx.is_cancelled() {
        ctx.cancel().await?;
        return Ok(());
    }
    let response = client.fetch_org_master(path).await?;
    let permissions = config::document_permissions(content_type, config, ctx.source_id(), None);
    let items = extract_items(&response);
    let mut count = 0i32;

    for item in &items {
        if ctx.is_cancelled() {
            ctx.cancel().await?;
            return Ok(());
        }
        let Some(id) = extract_stable_id(item) else {
            warn!(
                content_type,
                "Skipping Darwinbox record without a stable ID"
            );
            continue;
        };

        let (title, safe_body) = mappers::format_org_master_item(item, content_type);
        let content = safe_body;
        let content_id = ctx.store_content(&content).await?;

        let document_id = format!("{external_prefix}:{id}");
        ctx.emit_event(ConnectorEvent::DocumentCreated {
            sync_run_id: ctx.sync_run_id().to_string(),
            source_id: ctx.source_id().to_string(),
            document_id: document_id.clone(),
            content_id,
            metadata: DocumentMetadata {
                title: Some(title.to_string()),
                author: None,
                created_at: None,
                updated_at: None,
                content_type: Some(content_type.to_string()),
                mime_type: Some("text/markdown".to_string()),
                size: Some(content.len().to_string()),
                url: None,
                path: None,
                extra: None,
            },
            permissions: permissions.clone(),
            attributes: Some(std::collections::HashMap::from([
                ("source_type".to_string(), json!("darwinbox")),
                ("content_type".to_string(), json!(content_type)),
            ])),
        })
        .await?;
        current_ids.insert(document_id);
        count += 1;
    }
    if count > 0 {
        ctx.increment_scanned(count).await?;
    }
    Ok(())
}

async fn sync_holidays(
    client: &DarwinboxClient,
    config: &DarwinboxSourceConfig,
    source_id: &str,
    ctx: &SyncContext,
    current_ids: &mut BTreeSet<String>,
) -> Result<()> {
    let employees = client.fetch_employees(None, None).await?;
    let Some(employee_no) = employees
        .employee_data
        .iter()
        .find_map(|employee| employee.employee_id.as_deref())
    else {
        warn!("Skipping Darwinbox holiday sync because no employee_id was available");
        return Ok(());
    };
    let year = Utc::now().format("%Y").to_string();
    let response = client.fetch_holiday_list(employee_no, &year).await?;
    let permissions = config::document_permissions("holiday", config, source_id, None);
    let items = extract_items(&response);
    let mut count = 0i32;

    for (_idx, item) in items.iter().enumerate() {
        if ctx.is_cancelled() {
            ctx.cancel().await?;
            return Ok(());
        }
        let (title, safe_body) = mappers::format_holiday_item(item);
        let Some(date) = item.get("holiday_date").and_then(JsonValue::as_str) else {
            warn!("Skipping Darwinbox holiday without holiday_date");
            continue;
        };
        let id = slugify(&format!("{date}-{title}"));
        let content_id = ctx.store_content(&safe_body).await?;

        let document_id = format!("darwinbox:holiday:{id}");
        ctx.emit_event(ConnectorEvent::DocumentCreated {
            sync_run_id: ctx.sync_run_id().to_string(),
            source_id: source_id.to_string(),
            document_id: document_id.clone(),
            content_id,
            metadata: DocumentMetadata {
                title: Some(title),
                author: None,
                created_at: None,
                updated_at: None,
                content_type: Some("holiday".to_string()),
                mime_type: Some("text/markdown".to_string()),
                size: Some(safe_body.len().to_string()),
                url: None,
                path: None,
                extra: None,
            },
            permissions: permissions.clone(),
            attributes: Some(std::collections::HashMap::from([
                ("source_type".to_string(), json!("darwinbox")),
                ("content_type".to_string(), json!("holiday")),
            ])),
        })
        .await?;
        current_ids.insert(document_id);
        count += 1;
    }
    if count > 0 {
        ctx.increment_scanned(count).await?;
    }
    Ok(())
}

/// Sync a typed collection using a safe mapper function that derives (title, safe_content)
/// from only known fields of the provider response.
async fn sync_typed_collection<'a, F>(
    _client: &DarwinboxClient,
    config: &DarwinboxSourceConfig,
    content_type: &'a str,
    external_prefix: &'a str,
    response: &'a JsonValue,
    source_id: &str,
    ctx: &SyncContext,
    mapper: F,
    current_ids: &mut BTreeSet<String>,
) -> Result<()>
where
    F: Fn(&JsonValue) -> (String, String), // (title, safe_content)
{
    let permissions = config::document_permissions(content_type, config, source_id, None);
    let items = extract_items(response);
    let mut count = 0i32;

    for item in &items {
        if ctx.is_cancelled() {
            ctx.cancel().await?;
            return Ok(());
        }
        let Some(stable_id) = extract_stable_id(item) else {
            warn!(
                content_type,
                "Skipping Darwinbox record without a stable ID"
            );
            continue;
        };
        let (title, safe_body) = mapper(item);
        let content_id = ctx.store_content(&safe_body).await?;

        let document_id = format!("{external_prefix}:{stable_id}");
        ctx.emit_event(ConnectorEvent::DocumentCreated {
            sync_run_id: ctx.sync_run_id().to_string(),
            source_id: source_id.to_string(),
            document_id: document_id.clone(),
            content_id,
            metadata: DocumentMetadata {
                title: Some(title),
                author: None,
                created_at: None,
                updated_at: None,
                content_type: Some(content_type.to_string()),
                mime_type: Some("text/markdown".to_string()),
                size: Some(safe_body.len().to_string()),
                url: None,
                path: None,
                extra: None,
            },
            permissions: permissions.clone(),
            attributes: Some(std::collections::HashMap::from([
                ("source_type".to_string(), json!("darwinbox")),
                ("content_type".to_string(), json!(content_type)),
            ])),
        })
        .await?;
        current_ids.insert(document_id);
        count += 1;
    }
    if count > 0 {
        ctx.increment_scanned(count).await?;
    }
    Ok(())
}

fn extract_stable_id(value: &JsonValue) -> Option<String> {
    let object = value.as_object()?;
    for key in [
        "id",
        "code",
        "job_id",
        "employee_id",
        "department_code",
        "designation_code",
        "work_area_code",
        "name",
    ] {
        if let Some(raw) = object.get(key) {
            if let Some(text) = raw.as_str().filter(|text| !text.trim().is_empty()) {
                return Some(slugify(text));
            }
            if raw.is_number() {
                return Some(raw.to_string());
            }
        }
    }
    None
}

fn extract_items(response: &JsonValue) -> Vec<JsonValue> {
    for key in [
        "data",
        "output",
        "records",
        "result",
        "results",
        "holiday_list",
    ] {
        if let Some(array) = response.get(key).and_then(JsonValue::as_array) {
            return array.clone();
        }
    }
    if let Some(array) = response.as_array() {
        return array.clone();
    }
    vec![response.clone()]
}

fn slugify(value: &str) -> String {
    value
        .trim()
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

async fn save_inventory_checkpoint(
    ctx: &SyncContext,
    checkpoint: &mut DarwinboxCheckpoint,
    policy_fingerprint: &str,
    current_ids: &BTreeSet<String>,
) -> Result<()> {
    checkpoint.policy_fingerprint = Some(policy_fingerprint.to_string());
    checkpoint.indexed_document_ids = current_ids.clone();
    ctx.save_checkpoint(json!(checkpoint)).await
}

fn module_since(
    checkpoint: &DarwinboxCheckpoint,
    key: DarwinboxSyncModuleKey,
    ctx: &SyncContext,
) -> Option<String> {
    if ctx.sync_mode() == SyncType::Full {
        return None;
    }
    checkpoint
        .modules
        .get(&key)
        .and_then(|module| module.watermark_ts.clone())
}

fn set_module_watermark(
    checkpoint: &mut DarwinboxCheckpoint,
    key: DarwinboxSyncModuleKey,
    watermark_ts: String,
) {
    checkpoint.modules.insert(
        key,
        ModuleCheckpoint {
            watermark_ts: Some(watermark_ts),
            in_progress: None,
        },
    );
}

fn to_darwinbox_timestamp_with_overlap(value: &str) -> Option<String> {
    let parsed = DateTime::parse_from_rfc3339(value).ok()?;
    let overlapped = parsed.with_timezone(&Utc) - Duration::seconds(INCREMENTAL_OVERLAP_SECONDS);
    Some(overlapped.format("%d-%m-%Y %H:%M:%S").to_string())
}
