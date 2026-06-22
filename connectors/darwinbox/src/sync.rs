use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use omni_connector_sdk::{
    ConnectorEvent, DocumentMetadata, DocumentPermissions, SyncContext, SyncType,
};
use serde_json::{json, Value as JsonValue};
use tracing::{info, warn};

use crate::client::DarwinboxClient;
use crate::config::DarwinboxSourceConfig;
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
    checkpoint.schema_version = 1;

    if config.sync_modules.employee_directory {
        let since = module_since(&checkpoint, DarwinboxSyncModuleKey::EmployeeDirectory, &ctx);
        sync_employee_directory(client, since.as_deref(), &ctx).await?;
        set_module_watermark(
            &mut checkpoint,
            DarwinboxSyncModuleKey::EmployeeDirectory,
            Utc::now().to_rfc3339(),
        );
        ctx.save_checkpoint(json!(checkpoint)).await?;
    }

    if config.sync_modules.deleted_employees {
        let since = module_since(&checkpoint, DarwinboxSyncModuleKey::DeletedEmployees, &ctx);
        sync_deleted_employees(client, since.as_deref(), &ctx).await?;
        set_module_watermark(
            &mut checkpoint,
            DarwinboxSyncModuleKey::DeletedEmployees,
            Utc::now().to_rfc3339(),
        );
        ctx.save_checkpoint(json!(checkpoint)).await?;
    }

    if config.sync_modules.org_masters {
        sync_org_masters(client, &ctx).await?;
        set_module_watermark(
            &mut checkpoint,
            DarwinboxSyncModuleKey::OrgMasters,
            Utc::now().to_rfc3339(),
        );
        ctx.save_checkpoint(json!(checkpoint)).await?;
    }

    if config.sync_modules.positions {
        let since = module_since(&checkpoint, DarwinboxSyncModuleKey::PositionMaster, &ctx);
        let provider_since = since
            .as_deref()
            .and_then(to_darwinbox_timestamp_with_overlap);
        let response = client
            .fetch_position_master(provider_since.as_deref())
            .await?;
        sync_generic_collection("position", "darwinbox:position", &response, &ctx).await?;
        set_module_watermark(
            &mut checkpoint,
            DarwinboxSyncModuleKey::PositionMaster,
            Utc::now().to_rfc3339(),
        );
        ctx.save_checkpoint(json!(checkpoint)).await?;
    }

    if config.sync_modules.holidays {
        sync_holidays(client, &ctx).await?;
        set_module_watermark(
            &mut checkpoint,
            DarwinboxSyncModuleKey::Holidays,
            Utc::now().to_rfc3339(),
        );
        ctx.save_checkpoint(json!(checkpoint)).await?;
    }

    if config.sync_modules.ats_jobs {
        let since = module_since(&checkpoint, DarwinboxSyncModuleKey::AtsJobs, &ctx);
        let provider_since = since
            .as_deref()
            .and_then(to_darwinbox_timestamp_with_overlap);
        let response = client.fetch_jobs(provider_since.as_deref()).await?;
        sync_generic_collection("job", "darwinbox:job", &response, &ctx).await?;
        set_module_watermark(
            &mut checkpoint,
            DarwinboxSyncModuleKey::AtsJobs,
            Utc::now().to_rfc3339(),
        );
        ctx.save_checkpoint(json!(checkpoint)).await?;
    }

    info!(source_id = ctx.source_id(), "Darwinbox sync completed");

    Ok(())
}

async fn sync_employee_directory(
    client: &DarwinboxClient,
    last_modified: Option<&str>,
    ctx: &SyncContext,
) -> Result<()> {
    let provider_last_modified = last_modified.and_then(to_darwinbox_timestamp_with_overlap);
    let response = client
        .fetch_employees(None, provider_last_modified.as_deref())
        .await
        .context("failed to fetch Darwinbox employee directory")?;

    let mut emitted = 0i32;
    let mut member_emails = Vec::new();

    for employee in response.employee_data {
        if ctx.is_cancelled() {
            ctx.cancel().await?;
            return Ok(());
        }

        let Some(document_id) = employee.external_id() else {
            warn!("Skipping Darwinbox employee without employee_id");
            continue;
        };

        let content = employee.content();
        let content_id = ctx
            .store_content(&content)
            .await
            .with_context(|| format!("failed to store content for {document_id}"))?;

        if let Some(event) = employee.to_event(
            ctx.sync_run_id().to_string(),
            ctx.source_id().to_string(),
            content_id,
        ) {
            ctx.emit_event(event).await?;
            emitted += 1;
        }

        if let Some(email) = employee.company_email_id.as_deref() {
            if !email.trim().is_empty() {
                member_emails.push(email.trim().to_ascii_lowercase());
            }
        }

        if emitted > 0 && emitted % 100 == 0 {
            ctx.increment_scanned(100).await?;
        }
    }

    if emitted % 100 != 0 {
        ctx.increment_scanned(emitted % 100).await?;
    }

    if !member_emails.is_empty() {
        ctx.emit_event(ConnectorEvent::GroupMembershipSync {
            sync_run_id: ctx.sync_run_id().to_string(),
            source_id: ctx.source_id().to_string(),
            group_email: format!("darwinbox:employees:{}", ctx.source_id()),
            group_name: Some("Darwinbox Employees".to_string()),
            member_emails,
        })
        .await?;
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

async fn sync_org_masters(client: &DarwinboxClient, ctx: &SyncContext) -> Result<()> {
    let endpoints = [
        (
            "department",
            "darwinbox:department",
            "/orgmasterapi/departmentlist",
        ),
        (
            "designation",
            "darwinbox:designation",
            "/orgmasterapi/designationlist",
        ),
        (
            "office_location",
            "darwinbox:office_location",
            "/orgmasterapi/officelocationlist",
        ),
        (
            "business_unit",
            "darwinbox:business_unit",
            "/orgmasterapi/businessunitlist",
        ),
        (
            "division",
            "darwinbox:division",
            "/orgmasterapi/divisionlist",
        ),
        (
            "cost_center",
            "darwinbox:cost_center",
            "/orgmasterapi/costcenterlist",
        ),
        (
            "group_company",
            "darwinbox:group_company",
            "/orgmasterapi/groupcompanylist",
        ),
    ];

    for (content_type, prefix, path) in endpoints {
        if ctx.is_cancelled() {
            ctx.cancel().await?;
            return Ok(());
        }
        let response = client.fetch_org_master(path).await?;
        sync_generic_collection(content_type, prefix, &response, ctx).await?;
    }

    Ok(())
}

async fn sync_holidays(client: &DarwinboxClient, ctx: &SyncContext) -> Result<()> {
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
    sync_generic_collection("holiday", "darwinbox:holiday", &response, ctx).await
}

async fn sync_generic_collection(
    content_type: &str,
    external_prefix: &str,
    response: &JsonValue,
    ctx: &SyncContext,
) -> Result<()> {
    let items = extract_items(response);
    let mut count = 0i32;
    for (idx, item) in items.iter().enumerate() {
        if ctx.is_cancelled() {
            ctx.cancel().await?;
            return Ok(());
        }
        let id = extract_stable_id(item).unwrap_or_else(|| idx.to_string());
        let title = extract_title(item).unwrap_or_else(|| format!("{content_type} {id}"));
        let content = format!(
            "# {title}\n\n```json\n{}\n```",
            serde_json::to_string_pretty(item)?
        );
        let content_id = ctx.store_content(&content).await?;
        ctx.emit_event(ConnectorEvent::DocumentCreated {
            sync_run_id: ctx.sync_run_id().to_string(),
            source_id: ctx.source_id().to_string(),
            document_id: format!("{external_prefix}:{id}"),
            content_id,
            metadata: DocumentMetadata {
                title: Some(title),
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
            permissions: DocumentPermissions {
                public: true,
                users: vec![],
                groups: vec![],
            },
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

fn extract_title(value: &JsonValue) -> Option<String> {
    let object = value.as_object()?;
    for key in [
        "name",
        "title",
        "job_title",
        "department_name",
        "designation_name",
        "location",
        "holiday_name",
    ] {
        if let Some(text) = object.get(key).and_then(JsonValue::as_str) {
            if !text.trim().is_empty() {
                return Some(text.to_string());
            }
        }
    }
    None
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
