use anyhow::{anyhow, Context, Result};
use axum::response::Response;
use chrono::{Datelike, Utc};
use omni_connector_sdk::{
    ActionContext, ActionDefinition, ActionMode, ActionResponse, ServiceCredential, SourceType,
};
use serde_json::{json, Value as JsonValue};

use crate::client::{
    ApplyLeaveRequest, DarwinboxClient, LeaveDecision, LeaveDecisionRequest, LeaveRequestsRequest,
    MonthlyAttendanceRequest, RevokeLeaveRequest,
};
use crate::config::DarwinboxSourceConfig;
use crate::credentials::DarwinboxCredentials;
use crate::models::EmployeeRecord;

pub fn action_definitions() -> Vec<ActionDefinition> {
    let source_types = vec![SourceType::Darwinbox];
    vec![
        read(
            "get_my_profile",
            "Get the calling employee's Darwinbox profile.",
            json!({ "type": "object", "properties": {}, "additionalProperties": false }),
            &source_types,
        ),
        read(
            "find_employee",
            "Find employees in the synced Darwinbox directory.",
            json!({ "type": "object", "properties": { "query": { "type": "string" } }, "required": ["query"], "additionalProperties": false }),
            &source_types,
        ),
        read(
            "get_my_leave_balance",
            "Get the calling employee's leave balances.",
            json!({ "type": "object", "properties": {}, "additionalProperties": false }),
            &source_types,
        ),
        read(
            "get_holiday_calendar",
            "Get holidays for a year/employee calendar.",
            json!({ "type": "object", "properties": { "year": { "type": "string" } }, "additionalProperties": false }),
            &source_types,
        ),
        write(
            "apply_my_leave",
            "Apply leave for the calling employee.",
            json!({ "type": "object", "properties": { "leave_name": { "type": "string" }, "message": { "type": "string" }, "from_date": { "type": "string" }, "to_date": { "type": "string" }, "is_half_day": { "type": "string", "enum": ["Yes", "No"], "default": "No" }, "is_paid_or_unpaid": { "type": "string", "enum": ["paid", "unpaid"], "default": "paid" } }, "required": ["leave_name", "message", "from_date", "to_date"], "additionalProperties": false }),
            &source_types,
        ),
        write(
            "revoke_my_leave",
            "Revoke leave for the calling employee.",
            json!({ "type": "object", "properties": { "leave_id": { "type": "string" }, "revoke_reason": { "type": "string" } }, "required": ["leave_id", "revoke_reason"], "additionalProperties": false }),
            &source_types,
        ),
        read(
            "get_my_leave_requests",
            "Get leave requests for the calling employee.",
            json!({ "type": "object", "properties": { "from": { "type": "string" }, "to": { "type": "string" }, "action": { "type": "string" } }, "required": ["from", "to"], "additionalProperties": false }),
            &source_types,
        ),
        read(
            "get_my_attendance",
            "Get attendance for the calling employee.",
            json!({ "type": "object", "properties": { "from_date": { "type": "string" }, "to_date": { "type": "string" }, "month": { "type": "string" } }, "additionalProperties": false }),
            &source_types,
        ),
        write(
            "regularize_my_attendance",
            "Submit backdated attendance regularization for the calling employee.",
            json!({ "type": "object", "properties": { "attendance": { "type": "object" } }, "required": ["attendance"], "additionalProperties": false }),
            &source_types,
        ),
        read(
            "get_my_timesheet",
            "Get timesheet entries for the calling employee.",
            json!({ "type": "object", "properties": { "from": { "type": "string" }, "to": { "type": "string" } }, "required": ["from", "to"], "additionalProperties": false }),
            &source_types,
        ),
        read(
            "list_pending_leave_approvals",
            "List pending leave approvals for a manager.",
            json!({ "type": "object", "properties": {}, "additionalProperties": false }),
            &source_types,
        ),
        write(
            "approve_leave_request",
            "Approve a leave request.",
            json!({ "type": "object", "properties": { "leave_id": { "type": "string" }, "employee_no": { "type": "string" }, "manager_message": { "type": "string" } }, "required": ["leave_id", "employee_no"], "additionalProperties": false }),
            &source_types,
        ),
        write(
            "reject_leave_request",
            "Reject a leave request.",
            json!({ "type": "object", "properties": { "leave_id": { "type": "string" }, "employee_no": { "type": "string" }, "manager_message": { "type": "string" } }, "required": ["leave_id", "employee_no"], "additionalProperties": false }),
            &source_types,
        ),
        read(
            "get_team_leave_calendar",
            "Get team leave calendar for a manager.",
            json!({ "type": "object", "properties": { "from": { "type": "string" }, "to": { "type": "string" } }, "required": ["from", "to"], "additionalProperties": false }),
            &source_types,
        ),
        read(
            "get_team_attendance_exceptions",
            "Get team attendance exceptions for a manager.",
            json!({ "type": "object", "properties": { "from_date": { "type": "string" }, "to_date": { "type": "string" } }, "required": ["from_date", "to_date"], "additionalProperties": false }),
            &source_types,
        ),
        read(
            "get_direct_report_profile",
            "Get a direct report's profile after manager authorization.",
            json!({ "type": "object", "properties": { "employee_no": { "type": "string" } }, "required": ["employee_no"], "additionalProperties": false }),
            &source_types,
        ),
        admin_write(
            "add_pending_employee",
            "Add a pending employee record.",
            json!({ "type": "object", "properties": { "employee": { "type": "object" } }, "required": ["employee"], "additionalProperties": false }),
            &source_types,
        ),
        admin_write(
            "activate_pending_employee",
            "Activate pending employees.",
            json!({ "type": "object", "properties": { "user_ids": { "type": "array", "items": { "type": "string" } } }, "required": ["user_ids"], "additionalProperties": false }),
            &source_types,
        ),
        admin_write(
            "update_employee_record",
            "Update an employee record.",
            json!({ "type": "object", "properties": { "employee": { "type": "object" } }, "required": ["employee"], "additionalProperties": false }),
            &source_types,
        ),
        admin_write(
            "update_employment_details",
            "Update employee employment details.",
            json!({ "type": "object", "properties": { "employment_details": { "type": "object" } }, "required": ["employment_details"], "additionalProperties": false }),
            &source_types,
        ),
        admin_write(
            "deactivate_employee",
            "Deactivate an employee.",
            json!({ "type": "object", "properties": { "employees": { "type": "array", "items": { "type": "object" } } }, "required": ["employees"], "additionalProperties": false }),
            &source_types,
        ),
        admin_write(
            "reactivate_employee",
            "Reactivate a deactivated employee.",
            json!({ "type": "object", "properties": { "employees": { "type": "array", "items": { "type": "object" } } }, "required": ["employees"], "additionalProperties": false }),
            &source_types,
        ),
        admin_write(
            "upload_employee_document",
            "Upload an employee document.",
            json!({ "type": "object", "properties": { "employee_no": { "type": "string" }, "document_type": { "type": "string" }, "attachment": { "type": "string" } }, "required": ["employee_no", "document_type", "attachment"], "additionalProperties": false }),
            &source_types,
        ),
        admin_read(
            "fetch_employee_history",
            "Fetch employee history.",
            json!({ "type": "object", "properties": { "from": { "type": "string" }, "to": { "type": "string" }, "filter_on_effective_date": { "type": "number" } }, "required": ["from", "to", "filter_on_effective_date"], "additionalProperties": false }),
            &source_types,
        ),
        admin_read(
            "fetch_report_ids",
            "Fetch available Darwinbox report IDs.",
            json!({ "type": "object", "properties": {}, "additionalProperties": false }),
            &source_types,
        ),
        admin_read(
            "run_report",
            "Run a Darwinbox report by ID.",
            json!({ "type": "object", "properties": { "report_id": { "type": "string" } }, "required": ["report_id"], "additionalProperties": false }),
            &source_types,
        ),
        read(
            "list_jobs",
            "List Darwinbox ATS jobs.",
            json!({ "type": "object", "properties": { "job_updated_timestamp_from": { "type": "string" } }, "additionalProperties": false }),
            &source_types,
        ),
        read(
            "get_job_detail",
            "Get Darwinbox ATS job details.",
            json!({ "type": "object", "properties": { "job_id": { "type": "string" } }, "required": ["job_id"], "additionalProperties": false }),
            &source_types,
        ),
        read(
            "get_candidates",
            "Fetch Darwinbox ATS candidates.",
            json!({ "type": "object", "properties": { "updated_from": { "type": "string" }, "updated_to": { "type": "string" }, "job_id": { "type": "string" } }, "additionalProperties": false }),
            &source_types,
        ),
        write(
            "tag_candidate",
            "Add tags to Darwinbox ATS candidate profiles.",
            json!({ "type": "object", "properties": { "candidate_ids": { "type": "array", "items": { "type": "string" } }, "tags": { "type": "array", "items": { "type": "string" } } }, "required": ["candidate_ids", "tags"], "additionalProperties": false }),
            &source_types,
        ),
        write(
            "reject_candidate",
            "Reject Darwinbox ATS candidates.",
            json!({ "type": "object", "properties": { "candidate_ids": { "type": "array", "items": { "type": "string" } }, "reason": { "type": "string" } }, "required": ["candidate_ids"], "additionalProperties": false }),
            &source_types,
        ),
        write(
            "create_requisition",
            "Create a Darwinbox requisition.",
            json!({ "type": "object", "properties": { "requisition": { "type": "object" } }, "required": ["requisition"], "additionalProperties": false }),
            &source_types,
        ),
        write(
            "archive_requisition",
            "Archive a Darwinbox requisition.",
            json!({ "type": "object", "properties": { "requisition_id": { "type": "string" }, "employee_id": { "type": "string" }, "reason": { "type": "string" } }, "required": ["requisition_id", "employee_id"], "additionalProperties": false }),
            &source_types,
        ),
    ]
}

pub async fn execute_action(
    action: &str,
    params: JsonValue,
    credentials: Option<ServiceCredential>,
) -> Result<Response> {
    let (client, config, params, action_context) = action_runtime(params, credentials)?;
    let result = match action {
        "get_my_profile" => {
            reject_identity_params(&params)?;
            let employee = resolve_calling_employee(&client, &action_context).await?;
            json!({ "employee": employee })
        }
        "find_employee" => {
            let query = required_str(&params, "query")?.to_ascii_lowercase();
            let employees = client.fetch_employees(None, None).await?.employee_data;
            let matches = employees
                .into_iter()
                .filter(|employee| {
                    employee
                        .display_name()
                        .to_ascii_lowercase()
                        .contains(&query)
                        || employee
                            .employee_id
                            .as_deref()
                            .unwrap_or_default()
                            .to_ascii_lowercase()
                            .contains(&query)
                        || employee
                            .company_email_id
                            .as_deref()
                            .unwrap_or_default()
                            .to_ascii_lowercase()
                            .contains(&query)
                        || employee
                            .department_name
                            .as_deref()
                            .unwrap_or_default()
                            .to_ascii_lowercase()
                            .contains(&query)
                })
                .take(20)
                .collect::<Vec<_>>();
            json!({ "employees": matches })
        }
        "get_my_leave_balance" => {
            reject_identity_params(&params)?;
            let employee = resolve_calling_employee(&client, &action_context).await?;
            let employee_no = employee_id(&employee)?;
            client.fetch_leave_balance(employee_no).await?
        }
        "get_holiday_calendar" => {
            reject_identity_params(&params)?;
            let employee = resolve_calling_employee(&client, &action_context).await?;
            let employee_no = employee_id(&employee)?;
            let year = params
                .get("year")
                .and_then(JsonValue::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| current_year(&config));
            client.fetch_holiday_list(employee_no, &year).await?
        }
        "apply_my_leave" => {
            reject_identity_params(&params)?;
            let employee = resolve_calling_employee(&client, &action_context).await?;
            let employee_no = employee_id(&employee)?;
            client
                .apply_leave(ApplyLeaveRequest {
                    employee_no: employee_no.to_string(),
                    leave_name: required_str(&params, "leave_name")?.to_string(),
                    message: required_str(&params, "message")?.to_string(),
                    from_date: required_str(&params, "from_date")?.to_string(),
                    to_date: required_str(&params, "to_date")?.to_string(),
                    is_half_day: optional_str(&params, "is_half_day")
                        .unwrap_or("No")
                        .to_string(),
                    is_paid_or_unpaid: optional_str(&params, "is_paid_or_unpaid")
                        .unwrap_or("paid")
                        .to_string(),
                })
                .await?
        }
        "revoke_my_leave" => {
            reject_identity_params(&params)?;
            let employee = resolve_calling_employee(&client, &action_context).await?;
            let employee_no = employee_id(&employee)?;
            client
                .revoke_leave(RevokeLeaveRequest {
                    employee_no: employee_no.to_string(),
                    leave_id: required_str(&params, "leave_id")?.to_string(),
                    revoke_reason: required_str(&params, "revoke_reason")?.to_string(),
                })
                .await?
        }
        "get_my_leave_requests" => {
            reject_identity_params(&params)?;
            let employee = resolve_calling_employee(&client, &action_context).await?;
            let employee_no = employee_id(&employee)?;
            client
                .fetch_leave_requests(LeaveRequestsRequest {
                    employee_nos: vec![employee_no.to_string()],
                    from: Some(required_str(&params, "from")?.to_string()),
                    to: Some(required_str(&params, "to")?.to_string()),
                    action: optional_str(&params, "action").unwrap_or("0").to_string(),
                })
                .await?
        }
        "get_my_attendance" => {
            reject_identity_params(&params)?;
            let employee = resolve_calling_employee(&client, &action_context).await?;
            let employee_no = employee_id(&employee)?;
            let month = optional_str(&params, "month")
                .map(str::to_string)
                .or_else(|| default_attendance_month(&params, &config));
            client
                .fetch_monthly_attendance(MonthlyAttendanceRequest {
                    employee_nos: vec![employee_no.to_string()],
                    from_date: optional_str(&params, "from_date").map(str::to_string),
                    to_date: optional_str(&params, "to_date").map(str::to_string),
                    month,
                })
                .await?
        }
        "regularize_my_attendance" => {
            reject_identity_params(&params)?;
            let employee = resolve_calling_employee(&client, &action_context).await?;
            let employee_no = employee_id(&employee)?;
            let attendance = params
                .get("attendance")
                .cloned()
                .ok_or_else(|| anyhow!("attendance is required"))?;
            reject_identity_payload(&attendance)?;
            client
                .regularize_attendance(employee_no, attendance)
                .await?
        }
        "get_my_timesheet" => {
            reject_identity_params(&params)?;
            let employee = resolve_calling_employee(&client, &action_context).await?;
            let employee_no = employee_id(&employee)?;
            client
                .fetch_timesheet(
                    employee_no,
                    required_str(&params, "from")?,
                    required_str(&params, "to")?,
                )
                .await?
        }
        "list_pending_leave_approvals" => {
            let reports = direct_reports(&client, &action_context).await?;
            let employee_nos = employee_ids(reports);
            client
                .fetch_leave_requests(LeaveRequestsRequest {
                    employee_nos,
                    from: None,
                    to: None,
                    action: "1".to_string(),
                })
                .await?
        }
        "approve_leave_request" | "reject_leave_request" => {
            let employee_no = required_str(&params, "employee_no")?;
            ensure_direct_report(&client, &action_context, employee_no).await?;
            let decision = if action == "approve_leave_request" {
                LeaveDecision::Approve
            } else {
                LeaveDecision::Reject
            };
            client
                .take_leave_decision(LeaveDecisionRequest {
                    employee_no: employee_no.to_string(),
                    leave_id: required_str(&params, "leave_id")?.to_string(),
                    decision,
                    manager_message: optional_str(&params, "manager_message").map(str::to_string),
                })
                .await?
        }
        "get_team_leave_calendar" => {
            let reports = direct_reports(&client, &action_context).await?;
            let employee_nos = employee_ids(reports);
            client
                .fetch_leave_requests(LeaveRequestsRequest {
                    employee_nos,
                    from: Some(required_str(&params, "from")?.to_string()),
                    to: Some(required_str(&params, "to")?.to_string()),
                    action: "2".to_string(),
                })
                .await?
        }
        "get_team_attendance_exceptions" => {
            let reports = direct_reports(&client, &action_context).await?;
            let employee_nos = employee_ids(reports);
            client
                .fetch_daily_attendance_roster(
                    employee_nos,
                    required_str(&params, "from_date")?,
                    required_str(&params, "to_date")?,
                )
                .await?
        }
        "get_direct_report_profile" => {
            let employee_no = required_str(&params, "employee_no")?;
            ensure_direct_report(&client, &action_context, employee_no).await?;
            let employee = client
                .fetch_employees(Some(vec![employee_no.to_string()]), None)
                .await?
                .employee_data;
            json!({ "employees": employee })
        }
        "add_pending_employee" => {
            ensure_enabled(config.action_modules.hr_operations, "hr_operations")?;
            client.post_json::<JsonValue>("/importapi/add", json!({ "employees": [params.get("employee").cloned().ok_or_else(|| anyhow!("employee is required"))?] }), false).await?
        }
        "activate_pending_employee" => {
            ensure_enabled(config.action_modules.hr_operations, "hr_operations")?;
            client.post_json::<JsonValue>("/importapi/activate", json!({ "user_ids": params.get("user_ids").cloned().ok_or_else(|| anyhow!("user_ids is required"))? }), false).await?
        }
        "update_employee_record" => {
            ensure_enabled(config.action_modules.hr_operations, "hr_operations")?;
            client.post_json::<JsonValue>("/importapi/update", json!({ "employees": [params.get("employee").cloned().ok_or_else(|| anyhow!("employee is required"))?] }), false).await?
        }
        "update_employment_details" => {
            ensure_enabled(config.action_modules.hr_operations, "hr_operations")?;
            client
                .post_json::<JsonValue>(
                    "/importapi/updateemploymentdetails",
                    params
                        .get("employment_details")
                        .cloned()
                        .ok_or_else(|| anyhow!("employment_details is required"))?,
                    false,
                )
                .await?
        }
        "deactivate_employee" => {
            ensure_enabled(config.action_modules.hr_operations, "hr_operations")?;
            client.post_json::<JsonValue>("/importapi/deactivate", json!({ "employees": params.get("employees").cloned().ok_or_else(|| anyhow!("employees is required"))? }), false).await?
        }
        "reactivate_employee" => {
            ensure_enabled(config.action_modules.hr_operations, "hr_operations")?;
            client.post_json::<JsonValue>("/importapi/undodeactivation", json!({ "employees": params.get("employees").cloned().ok_or_else(|| anyhow!("employees is required"))? }), false).await?
        }
        "upload_employee_document" => {
            ensure_enabled(config.action_modules.hr_operations, "hr_operations")?;
            client
                .post_json::<JsonValue>(
                    "/Employeedocs/StandardDoc",
                    json!({
                        "employee_no": required_str(&params, "employee_no")?,
                        "type": required_str(&params, "document_type")?,
                        "attachment": required_str(&params, "attachment")?
                    }),
                    false,
                )
                .await?
        }
        "fetch_employee_history" => {
            ensure_enabled(config.action_modules.hr_operations, "hr_operations")?;
            client.post_json::<JsonValue>("/UpdateEmployeeDetails/employeehistory", json!({
                "from": required_str(&params, "from")?,
                "to": required_str(&params, "to")?,
                "filter_on_effective_date": params.get("filter_on_effective_date").cloned().unwrap_or(json!(0))
            }), false).await?
        }
        "fetch_report_ids" => {
            ensure_enabled(config.action_modules.reports, "reports")?;
            client
                .post_json::<JsonValue>("/reportsbuilderapi/reportids", json!({}), false)
                .await?
        }
        "run_report" => {
            ensure_enabled(config.action_modules.reports, "reports")?;
            client
                .post_json::<JsonValue>(
                    "/reportsbuilderapi/reportdatav2",
                    json!({ "report_id": required_str(&params, "report_id")? }),
                    false,
                )
                .await?
        }
        "list_jobs" => {
            ensure_enabled(config.action_modules.ats, "ats")?;
            client
                .fetch_jobs(
                    params
                        .get("job_updated_timestamp_from")
                        .and_then(JsonValue::as_str),
                )
                .await?
        }
        "get_job_detail" => {
            ensure_enabled(config.action_modules.ats, "ats")?;
            client
                .post_json::<JsonValue>(
                    "/JobsApiv3/Jobdetail",
                    json!({ "job_id": required_str(&params, "job_id")? }),
                    false,
                )
                .await?
        }
        "get_candidates" => {
            ensure_enabled(config.action_modules.ats, "ats")?;
            client
                .post_json::<JsonValue>("/JobsApiv3/BulkCandidatesData", params.clone(), false)
                .await?
        }
        "tag_candidate" => {
            ensure_enabled(config.action_modules.ats, "ats")?;
            client.post_json::<JsonValue>("/JobsApiv2/candidatetag", json!({ "candidate_ids": params.get("candidate_ids").cloned().ok_or_else(|| anyhow!("candidate_ids is required"))?, "tags": params.get("tags").cloned().ok_or_else(|| anyhow!("tags is required"))? }), false).await?
        }
        "reject_candidate" => {
            ensure_enabled(config.action_modules.ats, "ats")?;
            client.post_json::<JsonValue>("/JobsApiv3/RejectCandidate", json!({ "candidate_ids": params.get("candidate_ids").cloned().ok_or_else(|| anyhow!("candidate_ids is required"))?, "reason": params.get("reason").cloned().unwrap_or(json!("")) }), false).await?
        }
        "create_requisition" => {
            ensure_enabled(config.action_modules.ats, "ats")?;
            client
                .post_json::<JsonValue>(
                    "/requisitionApi/createRequisition",
                    params
                        .get("requisition")
                        .cloned()
                        .ok_or_else(|| anyhow!("requisition is required"))?,
                    false,
                )
                .await?
        }
        "archive_requisition" => {
            ensure_enabled(config.action_modules.ats, "ats")?;
            client.post_json::<JsonValue>("/requisitionApi/archiveRequisition", json!({ "requisition_id": required_str(&params, "requisition_id")?, "employee_id": required_str(&params, "employee_id")?, "reason": params.get("reason").and_then(JsonValue::as_str).unwrap_or("") }), false).await?
        }
        _ => return Ok(ActionResponse::not_supported(action).into_response()),
    };

    Ok(ActionResponse::success(result).into_response())
}

fn action_runtime(
    params: JsonValue,
    credentials: Option<ServiceCredential>,
) -> Result<(
    DarwinboxClient,
    DarwinboxSourceConfig,
    JsonValue,
    ActionContext,
)> {
    let config: DarwinboxSourceConfig = serde_json::from_value(params.clone())
        .context("Darwinbox source config was not merged into action params")?;
    let creds = credentials.ok_or_else(|| anyhow!("Darwinbox credentials are required"))?;
    let darwinbox_creds: DarwinboxCredentials = serde_json::from_value(creds.credentials)
        .context("failed to decode Darwinbox credentials")?;
    let context = params
        .get("_omni_action_context")
        .cloned()
        .map(serde_json::from_value::<ActionContext>)
        .transpose()
        .context("invalid action context")?
        .ok_or_else(|| anyhow!("Darwinbox action requires authenticated caller context"))?;
    let client = DarwinboxClient::new(&config, darwinbox_creds)?;
    Ok((client, config, params, context))
}

fn ensure_enabled(enabled: bool, module: &str) -> Result<()> {
    if enabled {
        Ok(())
    } else {
        Err(anyhow!(
            "Darwinbox action module '{module}' is disabled for this source"
        ))
    }
}

fn reject_identity_params(params: &JsonValue) -> Result<()> {
    for key in IDENTITY_PARAM_KEYS {
        if params.get(key).is_some() {
            return Err(identity_field_error(key));
        }
    }
    Ok(())
}

fn reject_identity_payload(value: &JsonValue) -> Result<()> {
    match value {
        JsonValue::Object(object) => {
            for (key, child) in object {
                if IDENTITY_PARAM_KEYS.contains(&key.as_str()) {
                    return Err(identity_field_error(key));
                }
                reject_identity_payload(child)?;
            }
        }
        JsonValue::Array(items) => {
            for item in items {
                reject_identity_payload(item)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn identity_field_error(key: &str) -> anyhow::Error {
    anyhow!("self-service Darwinbox actions do not accept caller-supplied identity field '{key}'")
}

const IDENTITY_PARAM_KEYS: &[&str] = &[
    "employee_id",
    "employee_no",
    "email",
    "company_email_id",
    "user_id",
];

async fn resolve_calling_employee(
    client: &DarwinboxClient,
    context: &ActionContext,
) -> Result<EmployeeRecord> {
    let caller_email = context
        .user_email()
        .ok_or_else(|| anyhow!("authenticated caller email is required"))?
        .to_ascii_lowercase();
    let employees = client.fetch_employees(None, None).await?.employee_data;
    employees
        .into_iter()
        .find(|employee| {
            employee
                .company_email_id
                .as_deref()
                .map(|email| email.eq_ignore_ascii_case(&caller_email))
                .unwrap_or(false)
        })
        .ok_or_else(|| anyhow!("no Darwinbox employee found for caller email {caller_email}"))
}

async fn direct_reports(
    client: &DarwinboxClient,
    context: &ActionContext,
) -> Result<Vec<EmployeeRecord>> {
    let manager = resolve_calling_employee(client, context).await?;
    let manager_id = employee_id(&manager)?.to_string();
    let employees = client.fetch_employees(None, None).await?.employee_data;
    Ok(employees
        .into_iter()
        .filter(|employee| {
            employee
                .direct_manager_employee_id
                .as_deref()
                .map(|id| id == manager_id)
                .unwrap_or(false)
        })
        .collect())
}

async fn ensure_direct_report(
    client: &DarwinboxClient,
    context: &ActionContext,
    employee_no: &str,
) -> Result<()> {
    if direct_reports(client, context)
        .await?
        .iter()
        .any(|employee| employee.employee_id.as_deref() == Some(employee_no))
    {
        return Ok(());
    }
    Err(anyhow!(
        "employee {employee_no} is not a direct report of the caller"
    ))
}

fn employee_id(employee: &EmployeeRecord) -> Result<&str> {
    employee
        .employee_id
        .as_deref()
        .filter(|id| !id.trim().is_empty())
        .ok_or_else(|| anyhow!("Darwinbox employee record has no employee_id"))
}

fn employee_ids(employees: Vec<EmployeeRecord>) -> Vec<String> {
    employees
        .into_iter()
        .filter_map(|employee| employee.employee_id)
        .collect()
}

fn required_str<'a>(params: &'a JsonValue, key: &str) -> Result<&'a str> {
    optional_str(params, key).ok_or_else(|| anyhow!("{key} is required"))
}

fn optional_str<'a>(params: &'a JsonValue, key: &str) -> Option<&'a str> {
    params
        .get(key)
        .and_then(JsonValue::as_str)
        .filter(|value| !value.trim().is_empty())
}

fn current_year(config: &DarwinboxSourceConfig) -> String {
    current_date_parts(config).0
}

fn default_attendance_month(params: &JsonValue, config: &DarwinboxSourceConfig) -> Option<String> {
    if optional_str(params, "from_date").is_some() || optional_str(params, "to_date").is_some() {
        None
    } else {
        Some(current_date_parts(config).1)
    }
}

fn current_date_parts(config: &DarwinboxSourceConfig) -> (String, String) {
    let timezone = config
        .default_timezone
        .as_deref()
        .and_then(|timezone| timezone.parse::<chrono_tz::Tz>().ok())
        .unwrap_or(chrono_tz::UTC);
    let now = Utc::now().with_timezone(&timezone);
    (
        now.year().to_string(),
        format!("{:04}-{:02}", now.year(), now.month()),
    )
}

fn read(
    name: &str,
    description: &str,
    input_schema: JsonValue,
    source_types: &[SourceType],
) -> ActionDefinition {
    action(
        name,
        description,
        input_schema,
        ActionMode::Read,
        false,
        source_types,
    )
}

fn write(
    name: &str,
    description: &str,
    input_schema: JsonValue,
    source_types: &[SourceType],
) -> ActionDefinition {
    action(
        name,
        description,
        input_schema,
        ActionMode::Write,
        false,
        source_types,
    )
}

fn admin_read(
    name: &str,
    description: &str,
    input_schema: JsonValue,
    source_types: &[SourceType],
) -> ActionDefinition {
    action(
        name,
        description,
        input_schema,
        ActionMode::Read,
        true,
        source_types,
    )
}

fn admin_write(
    name: &str,
    description: &str,
    input_schema: JsonValue,
    source_types: &[SourceType],
) -> ActionDefinition {
    action(
        name,
        description,
        input_schema,
        ActionMode::Write,
        true,
        source_types,
    )
}

fn action(
    name: &str,
    description: &str,
    input_schema: JsonValue,
    mode: ActionMode,
    admin_only: bool,
    source_types: &[SourceType],
) -> ActionDefinition {
    ActionDefinition {
        name: name.to_string(),
        description: description.to_string(),
        input_schema,
        mode,
        source_types: source_types.to_vec(),
        admin_only,
        hidden: false,
    }
}
