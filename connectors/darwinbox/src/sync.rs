use std::collections::{BTreeSet, HashSet};

use anyhow::{Context, Result, bail};
use chrono::Utc;
use omni_connector_sdk::{ConnectorEvent, DocumentMetadata, SyncContext};
use serde_json::{Value as JsonValue, json};
use tracing::{info, warn};

use crate::client::DarwinboxClient;
use crate::config::{self, DarwinboxSourceConfig};
use crate::mappers;
use crate::models::{DarwinboxCheckpoint, DarwinboxSyncModuleKey, ModuleCheckpoint};

/// Schema version for per-person events and successfully synced email state.
const CURRENT_CHECKPOINT_SCHEMA: u16 = 4;

fn removed_person_emails(previous: &BTreeSet<String>, current: &BTreeSet<String>) -> Vec<String> {
    previous.difference(current).cloned().collect()
}

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

    // Bump schema for PersonSync-based checkpoint and discard state for
    // document-producing modules that are unavailable in this version.
    checkpoint.schema_version = CURRENT_CHECKPOINT_SCHEMA;
    checkpoint
        .modules
        .retain(|module, _| *module == DarwinboxSyncModuleKey::EmployeeDirectory);

    if config.sync_modules.employee_directory {
        let response = client
            .fetch_employees(None, None)
            .await
            .context("failed to fetch Darwinbox employee directory")?;

        // A missing status is not a confirmed complete response, so it must
        // never drive per-person removals.
        let status = response
            .status
            .ok_or_else(|| anyhow::anyhow!("Darwinbox employee API response omitted status"))?;
        if status != 1 && status != 200 {
            bail!("Darwinbox employee API returned non-success status {status}");
        }

        if ctx.is_cancelled() {
            ctx.cancel().await?;
            return Ok(());
        }

        let people = response.to_person_sync_records(&config.employee_fields, |employee| {
            employee
                .employee_id
                .as_deref()
                .is_some_and(|id| !id.trim().is_empty())
                && config.is_employee_in_scope(employee)
        })?;
        let mut external_ids = HashSet::with_capacity(people.len());
        let mut current_emails = BTreeSet::new();
        for person in &people {
            if !external_ids.insert(person.external_id.clone()) {
                bail!(
                    "Darwinbox employee API returned duplicate employee_id {}",
                    person.external_id
                );
            }
            if !current_emails.insert(person.email.clone()) {
                bail!("Darwinbox employee API returned duplicate company email");
            }
        }

        for person in people {
            if ctx.is_cancelled() {
                ctx.cancel().await?;
                return Ok(());
            }
            ctx.emit_event(ConnectorEvent::PersonSync {
                sync_run_id: ctx.sync_run_id().to_string(),
                source_id: ctx.source_id().to_string(),
                person,
            })
            .await?;
        }
        if ctx.is_cancelled() {
            ctx.cancel().await?;
            return Ok(());
        }
        for email in removed_person_emails(&checkpoint.synced_person_emails, &current_emails) {
            if ctx.is_cancelled() {
                ctx.cancel().await?;
                return Ok(());
            }
            ctx.emit_event(ConnectorEvent::PersonDeleted {
                sync_run_id: ctx.sync_run_id().to_string(),
                source_id: ctx.source_id().to_string(),
                email,
            })
            .await?;
        }
        ctx.increment_scanned(current_emails.len() as i32).await?;
        checkpoint.synced_person_emails = current_emails;
        set_module_watermark(
            &mut checkpoint,
            DarwinboxSyncModuleKey::EmployeeDirectory,
            Utc::now().to_rfc3339(),
        );
        if ctx.is_cancelled() {
            ctx.cancel().await?;
            return Ok(());
        }
        ctx.save_checkpoint(json!(checkpoint)).await?;
    } else {
        if ctx.is_cancelled() {
            ctx.cancel().await?;
            return Ok(());
        }
        for email in &checkpoint.synced_person_emails {
            if ctx.is_cancelled() {
                ctx.cancel().await?;
                return Ok(());
            }
            ctx.emit_event(ConnectorEvent::PersonDeleted {
                sync_run_id: ctx.sync_run_id().to_string(),
                source_id: ctx.source_id().to_string(),
                email: email.clone(),
            })
            .await?;
        }
        checkpoint.synced_person_emails.clear();
        checkpoint
            .modules
            .remove(&DarwinboxSyncModuleKey::EmployeeDirectory);
        if ctx.is_cancelled() {
            ctx.cancel().await?;
            return Ok(());
        }
        ctx.save_checkpoint(json!(checkpoint)).await?;
    }

    // Employee removal is derived from the complete Employee Master response;
    // no deleted-employee endpoint is required.

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
        )
        .await?;
    }

    // Positions
    if config.sync_modules.positions {
        let response = client.fetch_position_master(None).await?;
        sync_typed_collection(
            config,
            "position",
            "darwinbox:position",
            &response,
            ctx.source_id(),
            &ctx,
            mappers::format_position_item,
        )
        .await?;
        set_module_watermark(
            &mut checkpoint,
            DarwinboxSyncModuleKey::PositionMaster,
            Utc::now().to_rfc3339(),
        );
        ctx.save_checkpoint(json!(checkpoint)).await?;
    }

    // Holidays
    if config.sync_modules.holidays {
        sync_holidays(client, config, ctx.source_id(), &ctx).await?;
        set_module_watermark(
            &mut checkpoint,
            DarwinboxSyncModuleKey::Holidays,
            Utc::now().to_rfc3339(),
        );
        ctx.save_checkpoint(json!(checkpoint)).await?;
    }

    // ATS jobs
    if config.sync_modules.ats_jobs {
        let response = client.fetch_jobs(None).await?;
        sync_typed_collection(
            config,
            "ats_job",
            "darwinbox:job",
            &response,
            ctx.source_id(),
            &ctx,
            mappers::format_ats_job_item,
        )
        .await?;
        set_module_watermark(
            &mut checkpoint,
            DarwinboxSyncModuleKey::AtsJobs,
            Utc::now().to_rfc3339(),
        );
        ctx.save_checkpoint(json!(checkpoint)).await?;
    }

    // No document-level deletion reconciliation needed — PersonSync is authoritative
    // for people data, and org-master modules use their own independent state.

    info!(source_id = ctx.source_id(), "Darwinbox sync completed");
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
    config: &DarwinboxSourceConfig,
    content_type: &'a str,
    external_prefix: &'a str,
    response: &'a JsonValue,
    source_id: &str,
    ctx: &SyncContext,
    mapper: F,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn complete_directory_diff_deletes_only_absent_emails() {
        let previous = ["ada@example.com".into(), "grace@example.com".into()]
            .into_iter()
            .collect();
        let current = ["ada@example.com".into()].into_iter().collect();
        assert_eq!(
            removed_person_emails(&previous, &current),
            ["grace@example.com"]
        );
    }
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
