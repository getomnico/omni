use std::collections::{BTreeSet, HashMap, HashSet};

use anyhow::{Result, bail};
use chrono::Utc;
use omni_connector_sdk::{ConnectorEvent, DocumentMetadata, SyncContext};
use serde_json::{Value as JsonValue, json};
use tracing::{info, warn};

use crate::client::{DarwinboxApiError, DarwinboxClient};
use crate::config::{self, APPROVED_EMPLOYEE_FIELDS, DarwinboxSourceConfig};
use crate::mappers;
use crate::models::{DarwinboxCheckpoint, DarwinboxSyncModuleKey, ModuleCheckpoint};

/// Schema version for per-person events and successfully synced email state.
const CURRENT_CHECKPOINT_SCHEMA: u16 = 4;

/// (content_type, external prefix, API path) for each org master entity.
const ORG_MASTERS: &[(&str, &str, &str)] = &[
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
    checkpoint.schema_version = CURRENT_CHECKPOINT_SCHEMA;

    // People directory — full Employee Master snapshot. The dataset key is the
    // access control; a provider-side denial (4xx) skips the module with a
    // warning instead of failing the run, and leaves the previous checkpoint
    // intact so removals are still derivable on a later successful run.
    let directory = match client.fetch_employees(None, None).await {
        Ok(response) => {
            // A missing status is not a confirmed complete response, so it
            // must never drive per-person removals.
            let status = response
                .status
                .ok_or_else(|| anyhow::anyhow!("Darwinbox employee API response omitted status"))?;
            if status != 1 && status != 200 {
                bail!("Darwinbox employee API returned non-success status {status}");
            }
            Some(response)
        }
        Err(DarwinboxApiError::NotPermitted { .. }) => {
            warn!(
                source_id = ctx.source_id(),
                "Skipping People directory sync: Darwinbox denied /masterapi/employee access"
            );
            None
        }
        Err(error) => return Err(error.into()),
    };

    if ctx.is_cancelled() {
        ctx.cancel().await?;
        return Ok(());
    }

    if let Some(response) = &directory {
        let people = response.to_person_sync_records(APPROVED_EMPLOYEE_FIELDS, |employee| {
            employee
                .employee_id
                .as_deref()
                .is_some_and(|id| !id.trim().is_empty())
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
    }

    // Org masters — every entity type is attempted; denied modules are
    // skipped with a warning.
    for (content_type, external_prefix, path) in ORG_MASTERS {
        let ran = run_module(content_type, || async {
            let response = client.fetch_org_master(path).await?;
            sync_org_master(
                config,
                content_type,
                external_prefix,
                &response,
                ctx.source_id(),
                &ctx,
            )
            .await
        })
        .await?;
        if ran {
            set_module_watermark(
                &mut checkpoint,
                DarwinboxSyncModuleKey::OrgMasters,
                Utc::now().to_rfc3339(),
            );
        }
    }

    // Positions
    let ran = run_module("position", || async {
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
        .await
    })
    .await?;
    if ran {
        set_module_watermark(
            &mut checkpoint,
            DarwinboxSyncModuleKey::PositionMaster,
            Utc::now().to_rfc3339(),
        );
    }

    // Holidays — reuse the directory snapshot for the employee id instead of
    // refetching the full Employee Master.
    let directory_employee_id = directory.as_ref().and_then(|response| {
        response
            .employee_data
            .iter()
            .find_map(|employee| employee.employee_id.as_deref())
    });
    if let Some(employee_id) = directory_employee_id {
        let ran = run_module("holiday", || async {
            sync_holidays(client, config, ctx.source_id(), &ctx, employee_id).await
        })
        .await?;
        if ran {
            set_module_watermark(
                &mut checkpoint,
                DarwinboxSyncModuleKey::Holidays,
                Utc::now().to_rfc3339(),
            );
        }
    } else {
        warn!(
            source_id = ctx.source_id(),
            "Skipping Darwinbox holiday sync: no employee_id was available from the Employee Master"
        );
    }

    // ATS jobs
    let ran = run_module("ats_job", || async {
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
        .await
    })
    .await?;
    if ran {
        set_module_watermark(
            &mut checkpoint,
            DarwinboxSyncModuleKey::AtsJobs,
            Utc::now().to_rfc3339(),
        );
    }

    if ctx.is_cancelled() {
        ctx.cancel().await?;
        return Ok(());
    }
    ctx.save_checkpoint(json!(checkpoint)).await?;

    info!(source_id = ctx.source_id(), "Darwinbox sync completed");
    Ok(())
}

/// Run a sync module, returning `false` when the provider denied access (4xx)
/// so the caller can warn and continue with the remaining modules. Non-denial
/// failures propagate and fail the run.
async fn run_module<F, Fut>(module: &str, f: F) -> Result<bool>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<()>>,
{
    match f().await {
        Ok(()) => Ok(true),
        Err(error) if is_not_permitted(&error) => {
            warn!("Skipping Darwinbox {module} sync: {error}");
            Ok(false)
        }
        Err(error) => Err(error),
    }
}

fn is_not_permitted(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<DarwinboxApiError>()
        .is_some_and(|api_error| matches!(api_error, DarwinboxApiError::NotPermitted { .. }))
}

/// Emit one document per org master record with entity attributes
/// (e.g. `department`, `department_code`) for the search operators.
async fn sync_org_master(
    config: &DarwinboxSourceConfig,
    content_type: &str,
    external_prefix: &str,
    response: &JsonValue,
    source_id: &str,
    ctx: &SyncContext,
) -> Result<()> {
    let permissions = config::document_permissions(content_type, config, source_id, None);
    let items = extract_items(response);
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
        let document =
            mappers::format_org_master_item(item, content_type_to_attribute_key(content_type));
        let content_id = ctx.store_content(&document.body).await?;

        let document_id = format!("{external_prefix}:{id}");
        emit_document(
            ctx,
            content_type,
            source_id,
            &document_id,
            content_id,
            &document,
            &permissions,
        )
        .await?;
        count += 1;
    }
    if count > 0 {
        ctx.increment_scanned(count).await?;
    }
    Ok(())
}

/// Map an org master content type to its search attribute key.
fn content_type_to_attribute_key(content_type: &str) -> &'static str {
    match content_type {
        "department" => "department",
        "designation" => "designation",
        "office_location" => "office_location",
        "business_unit" => "business_unit",
        "division" => "division",
        "cost_center" => "cost_center",
        "group_company" => "group_company",
        _ => "org_master",
    }
}

async fn sync_holidays(
    client: &DarwinboxClient,
    config: &DarwinboxSourceConfig,
    source_id: &str,
    ctx: &SyncContext,
    employee_id: &str,
) -> Result<()> {
    let year = Utc::now().format("%Y").to_string();
    let response = client.fetch_holiday_list(employee_id, &year).await?;
    let permissions = config::document_permissions("holiday", config, source_id, None);
    let mut count = 0i32;

    for item in &response.holidays {
        if ctx.is_cancelled() {
            ctx.cancel().await?;
            return Ok(());
        }
        let document = mappers::format_holiday_item(item);
        let id = slugify(&format!("{}-{}", item.date, document.title));
        let content_id = ctx.store_content(&document.body).await?;

        let document_id = format!("darwinbox:holiday:{id}");
        emit_document(
            ctx,
            "holiday",
            source_id,
            &document_id,
            content_id,
            &document,
            &permissions,
        )
        .await?;
        count += 1;
    }
    if count > 0 {
        ctx.increment_scanned(count).await?;
    }
    Ok(())
}

/// Sync a typed collection using a safe mapper function that derives a
/// `SafeDocument` (title, content, attributes) from known fields only.
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
    F: Fn(&JsonValue) -> mappers::SafeDocument,
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
        let document = mapper(item);
        let content_id = ctx.store_content(&document.body).await?;

        let document_id = format!("{external_prefix}:{stable_id}");
        emit_document(
            ctx,
            content_type,
            source_id,
            &document_id,
            content_id,
            &document,
            &permissions,
        )
        .await?;
        count += 1;
    }
    if count > 0 {
        ctx.increment_scanned(count).await?;
    }
    Ok(())
}

async fn emit_document(
    ctx: &SyncContext,
    content_type: &str,
    source_id: &str,
    document_id: &str,
    content_id: String,
    document: &mappers::SafeDocument,
    permissions: &omni_connector_sdk::DocumentPermissions,
) -> Result<()> {
    ctx.emit_event(ConnectorEvent::DocumentCreated {
        sync_run_id: ctx.sync_run_id().to_string(),
        source_id: source_id.to_string(),
        document_id: document_id.to_string(),
        content_id,
        metadata: DocumentMetadata {
            title: Some(document.title.clone()),
            author: None,
            created_at: None,
            updated_at: None,
            content_type: Some(content_type.to_string()),
            mime_type: Some("text/markdown".to_string()),
            size: Some(document.body.len().to_string()),
            url: None,
            path: None,
            extra: None,
        },
        permissions: permissions.clone(),
        attributes: Some(doc_attributes(content_type, &document.attributes)),
    })
    .await
}

fn doc_attributes(content_type: &str, entity: &[(String, String)]) -> HashMap<String, JsonValue> {
    let mut map = HashMap::from([
        ("source_type".to_string(), json!("darwinbox")),
        ("content_type".to_string(), json!(content_type)),
    ]);
    for (key, value) in entity {
        if !value.trim().is_empty() {
            map.insert(key.clone(), json!(value));
        }
    }
    map
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
    use omni_connector_sdk::{SdkClient, SourceType, SyncType};
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

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

    #[tokio::test]
    async fn holiday_sync_emits_documents_from_real_api_field_names() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/leavesactionapi/holidaylist"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "status": 1,
                "holidays": [{
                    "id": "a68f996eb7bec5",
                    "name": "Independence Day",
                    "date": "2026-08-15",
                    "year": "2026",
                    "holiday_repeats": "No",
                    "is_optional": "No",
                    "is_national": "Yes"
                }],
                "errors": [],
                "message": "Successfully Loaded All Holidays"
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/sdk/content"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"content_id": "c-1"})))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/sdk/events/batch"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/sdk/sync/run-1/scanned"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let config: DarwinboxSourceConfig = serde_json::from_value(json!({
            "base_url": server.uri(),
            "authorization": { "participant_mode": "all" }
        }))
        .unwrap();
        let credentials = crate::credentials::DarwinboxCredentials::Basic {
            username: "api-user".to_string(),
            password: "secret".to_string(),
            api_key: "api-key".to_string(),
            dataset_key: "dataset-key".to_string(),
        };
        let client = DarwinboxClient::new(&config, credentials).unwrap();
        let ctx = SyncContext::new(
            SdkClient::new(&server.uri()),
            "run-1".to_string(),
            "source-1".to_string(),
            SourceType::Darwinbox,
            SyncType::Full,
            Arc::new(AtomicBool::new(false)),
        );

        sync_holidays(&client, &config, "source-1", &ctx, "EMP001")
            .await
            .unwrap();
        ctx.flush().await.unwrap();

        let batch = server
            .received_requests()
            .await
            .unwrap()
            .into_iter()
            .find(|request| request.url.path() == "/sdk/events/batch")
            .expect("events batch should be emitted");
        let body: JsonValue = serde_json::from_slice(&batch.body).unwrap();
        let events = body["events"]
            .as_array()
            .expect("batch should carry events");
        assert_eq!(events.len(), 1);
        let event = &events[0];
        assert_eq!(event["type"], "document_created");
        assert_eq!(
            event["document_id"],
            "darwinbox:holiday:2026-08-15-independence-day"
        );
        assert_eq!(event["metadata"]["title"], "Independence Day");
        assert_eq!(event["metadata"]["content_type"], "holiday");
    }

    #[tokio::test]
    async fn holiday_sync_fails_loudly_on_mismatched_envelope() {
        // A shape without the `holidays` key (the pre-fix fallback path) must
        // fail the module, not silently produce zero documents.
        for body in [
            json!({ "status": 1, "result": [] }),
            json!({ "holidays": "oops" }),
            json!({ "status": 1, "holidays": [{ "name": "No Date" }] }),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/leavesactionapi/holidaylist"))
                .respond_with(ResponseTemplate::new(200).set_body_json(body))
                .mount(&server)
                .await;

            let config: DarwinboxSourceConfig = serde_json::from_value(json!({
                "base_url": server.uri(),
                "authorization": { "participant_mode": "all" }
            }))
            .unwrap();
            let credentials = crate::credentials::DarwinboxCredentials::Basic {
                username: "api-user".to_string(),
                password: "secret".to_string(),
                api_key: "api-key".to_string(),
                dataset_key: "dataset-key".to_string(),
            };
            let client = DarwinboxClient::new(&config, credentials).unwrap();
            let ctx = SyncContext::new(
                SdkClient::new(&server.uri()),
                "run-1".to_string(),
                "source-1".to_string(),
                SourceType::Darwinbox,
                SyncType::Full,
                Arc::new(AtomicBool::new(false)),
            );

            let error = sync_holidays(&client, &config, "source-1", &ctx, "EMP001")
                .await
                .expect_err("mismatched holiday envelope must fail the module");
            assert!(
                format!("{error}").contains("Darwinbox API request failed"),
                "unexpected error: {error}"
            );
        }
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
