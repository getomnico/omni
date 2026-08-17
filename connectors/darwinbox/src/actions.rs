use anyhow::{Context, Result, anyhow};
use axum::response::Response;
use chrono::{Datelike, Utc};
use omni_connector_sdk::{
    ActionDefinition, ActionMode, ActionResponse, ServiceCredential, Source, SourceType,
};
use serde_json::{Value as JsonValue, json};

use crate::client::{
    ApplyLeaveRequest, DarwinboxApiError, DarwinboxClient, LeaveDecision, LeaveDecisionRequest,
    LeaveRequestsRequest, MonthlyAttendanceRequest, RevokeLeaveRequest,
};
use crate::config::DarwinboxSourceConfig;
use crate::credentials::DarwinboxCredentials;
use crate::models::EmployeeRecord;

/// Action policy table mapping every registered action to its classification.
pub struct ActionPolicy {
    pub name: &'static str,
    pub module: &'static str,
    pub mode: ActionMode,
    pub audience: &'static str,
    pub is_write: bool,
    /// Whether this action appears in the manifest/UI and can be selected.
    /// Unavailable actions are still recognized at execution to return a
    /// clear error, but they cannot be configured, listed, or executed
    /// through normal paths.
    pub available: bool,
}

pub fn action_endpoints(action: &str) -> &'static [&'static str] {
    match action {
        "get_my_profile" | "find_employee" => &["/masterapi/employee"],
        "get_my_leave_balance" => &["/masterapi/employee", "/leavesactionapi/leavebalance"],
        "get_holiday_calendar" => &["/masterapi/employee", "/leavesactionapi/holidaylist"],
        "get_my_leave_requests" => &[
            "/masterapi/employee",
            "/leavesactionapi/leaveActionTakenLeaves",
        ],
        "get_my_attendance" => &["/masterapi/employee", "/AttendanceDataApi/monthly"],
        "get_my_timesheet" => &[
            "/masterapi/employee",
            "/attendanceDataApi/timesheetdatewise",
        ],
        "apply_my_leave" | "revoke_my_leave" => {
            &["/masterapi/employee", "/leavesactionapi/importleave"]
        }
        "list_pending_leave_approvals" | "get_team_leave_calendar" => &[
            "/masterapi/employee",
            "/leavesactionapi/leaveActionTakenLeaves",
        ],
        "approve_leave_request" | "reject_leave_request" => {
            &["/masterapi/employee", "/leavesactionapi/leaveaction"]
        }
        _ => &[],
    }
}

/// All registered actions with their policy metadata.
pub fn action_policies() -> &'static [ActionPolicy] {
    &[
        // Self-service (employee reads) — all available
        ActionPolicy {
            name: "get_my_profile",
            module: "employee_self_service",
            mode: ActionMode::Read,
            audience: "self",
            is_write: false,
            available: true,
        },
        ActionPolicy {
            name: "get_my_leave_balance",
            module: "employee_self_service",
            mode: ActionMode::Read,
            audience: "self",
            is_write: false,
            available: true,
        },
        ActionPolicy {
            name: "get_holiday_calendar",
            module: "employee_self_service",
            mode: ActionMode::Read,
            audience: "self",
            is_write: false,
            available: true,
        },
        ActionPolicy {
            name: "get_my_leave_requests",
            module: "employee_self_service",
            mode: ActionMode::Read,
            audience: "self",
            is_write: false,
            available: true,
        },
        ActionPolicy {
            name: "get_my_attendance",
            module: "employee_self_service",
            mode: ActionMode::Read,
            audience: "self",
            is_write: false,
            available: true,
        },
        ActionPolicy {
            name: "get_my_timesheet",
            module: "employee_self_service",
            mode: ActionMode::Read,
            audience: "self",
            is_write: false,
            available: true,
        },
        // Self-service (employee writes) — available but require write-mode
        ActionPolicy {
            name: "apply_my_leave",
            module: "employee_self_service",
            mode: ActionMode::Write,
            audience: "self",
            is_write: true,
            available: true,
        },
        ActionPolicy {
            name: "revoke_my_leave",
            module: "employee_self_service",
            mode: ActionMode::Write,
            audience: "self",
            is_write: true,
            available: true,
        },
        // Unavailable: single blocked action
        ActionPolicy {
            name: "regularize_my_attendance",
            module: "employee_self_service",
            mode: ActionMode::Write,
            audience: "self",
            is_write: true,
            available: false,
        },
        // Manager workflows — leave reads, strictly scoped to the caller's
        // direct reports via direct_reports().
        ActionPolicy {
            name: "list_pending_leave_approvals",
            module: "manager_workflows",
            mode: ActionMode::Read,
            audience: "manager",
            is_write: false,
            available: true,
        },
        ActionPolicy {
            name: "get_team_leave_calendar",
            module: "manager_workflows",
            mode: ActionMode::Read,
            audience: "manager",
            is_write: false,
            available: true,
        },
        ActionPolicy {
            name: "get_team_attendance_exceptions",
            module: "manager_workflows",
            mode: ActionMode::Read,
            audience: "manager",
            is_write: false,
            available: false,
        },
        ActionPolicy {
            name: "get_direct_report_profile",
            module: "manager_workflows",
            mode: ActionMode::Read,
            audience: "manager",
            is_write: false,
            available: false,
        },
        // Manager writes — target employee must be a direct report
        // (ensure_direct_report) and the source must not be read-only.
        ActionPolicy {
            name: "approve_leave_request",
            module: "manager_workflows",
            mode: ActionMode::Write,
            audience: "manager",
            is_write: true,
            available: true,
        },
        ActionPolicy {
            name: "reject_leave_request",
            module: "manager_workflows",
            mode: ActionMode::Write,
            audience: "manager",
            is_write: true,
            available: true,
        },
        // Directory (available, no module)
        ActionPolicy {
            name: "find_employee",
            module: "",
            mode: ActionMode::Read,
            audience: "self",
            is_write: false,
            available: true,
        },
        // HR operations — unavailable
        ActionPolicy {
            name: "fetch_employee_history",
            module: "hr_operations",
            mode: ActionMode::Read,
            audience: "hr_admin",
            is_write: false,
            available: false,
        },
        ActionPolicy {
            name: "add_pending_employee",
            module: "hr_operations",
            mode: ActionMode::Write,
            audience: "hr_admin",
            is_write: true,
            available: false,
        },
        ActionPolicy {
            name: "activate_pending_employee",
            module: "hr_operations",
            mode: ActionMode::Write,
            audience: "hr_admin",
            is_write: true,
            available: false,
        },
        ActionPolicy {
            name: "update_employee_record",
            module: "hr_operations",
            mode: ActionMode::Write,
            audience: "hr_admin",
            is_write: true,
            available: false,
        },
        ActionPolicy {
            name: "update_employment_details",
            module: "hr_operations",
            mode: ActionMode::Write,
            audience: "hr_admin",
            is_write: true,
            available: false,
        },
        ActionPolicy {
            name: "deactivate_employee",
            module: "hr_operations",
            mode: ActionMode::Write,
            audience: "hr_admin",
            is_write: true,
            available: false,
        },
        ActionPolicy {
            name: "reactivate_employee",
            module: "hr_operations",
            mode: ActionMode::Write,
            audience: "hr_admin",
            is_write: true,
            available: false,
        },
        ActionPolicy {
            name: "upload_employee_document",
            module: "hr_operations",
            mode: ActionMode::Write,
            audience: "hr_admin",
            is_write: true,
            available: false,
        },
        // Reports — unavailable until allowed_report_ids enforcement is added
        ActionPolicy {
            name: "fetch_report_ids",
            module: "reports",
            mode: ActionMode::Read,
            audience: "",
            is_write: false,
            available: false,
        },
        ActionPolicy {
            name: "run_report",
            module: "reports",
            mode: ActionMode::Read,
            audience: "",
            is_write: false,
            available: false,
        },
        // ATS — unavailable until audience policy is safe
        ActionPolicy {
            name: "list_jobs",
            module: "ats",
            mode: ActionMode::Read,
            audience: "recruiter",
            is_write: false,
            available: false,
        },
        ActionPolicy {
            name: "get_job_detail",
            module: "ats",
            mode: ActionMode::Read,
            audience: "recruiter",
            is_write: false,
            available: false,
        },
        ActionPolicy {
            name: "get_candidates",
            module: "ats",
            mode: ActionMode::Read,
            audience: "recruiter",
            is_write: false,
            available: false,
        },
        ActionPolicy {
            name: "tag_candidate",
            module: "ats",
            mode: ActionMode::Write,
            audience: "recruiter",
            is_write: true,
            available: false,
        },
        ActionPolicy {
            name: "reject_candidate",
            module: "ats",
            mode: ActionMode::Write,
            audience: "recruiter",
            is_write: true,
            available: false,
        },
        ActionPolicy {
            name: "create_requisition",
            module: "ats",
            mode: ActionMode::Write,
            audience: "recruiter",
            is_write: true,
            available: false,
        },
        ActionPolicy {
            name: "archive_requisition",
            module: "ats",
            mode: ActionMode::Write,
            audience: "recruiter",
            is_write: true,
            available: false,
        },
    ]
}

/// Look up the policy for a given action name.
pub fn find_action_policy(action: &str) -> Option<&'static ActionPolicy> {
    action_policies().iter().find(|p| p.name == action)
}

/// Returns true if the action is available for selection and execution.
pub fn is_action_available(action: &str) -> bool {
    find_action_policy(action).map_or(false, |p| p.available)
}

/// Returns list of currently available actions only.
pub fn available_action_names() -> Vec<&'static str> {
    action_policies()
        .iter()
        .filter(|p| p.available)
        .map(|p| p.name)
        .collect()
}

pub fn action_definitions() -> Vec<ActionDefinition> {
    let source_types = vec![SourceType::Darwinbox];
    let mut definitions = Vec::new();

    // Only emit definitions for available actions
    for policy in action_policies() {
        if !policy.available {
            continue;
        }
        let def = match policy.name {
            "get_my_profile" => read(
                "get_my_profile",
                "Get the calling employee's Darwinbox profile.",
                json!({ "type": "object", "properties": {}, "additionalProperties": false }),
                &source_types,
            ),
            "find_employee" => read(
                "find_employee",
                "Find employees in the synced Darwinbox directory.",
                json!({ "type": "object", "properties": { "query": { "type": "string" } }, "required": ["query"], "additionalProperties": false }),
                &source_types,
            ),
            "get_my_leave_balance" => read(
                "get_my_leave_balance",
                "Get the calling employee's leave balances.",
                json!({ "type": "object", "properties": {}, "additionalProperties": false }),
                &source_types,
            ),
            "get_holiday_calendar" => read(
                "get_holiday_calendar",
                "Get holidays for a year/employee calendar.",
                json!({ "type": "object", "properties": { "year": { "type": "string" } }, "additionalProperties": false }),
                &source_types,
            ),
            "get_my_leave_requests" => read(
                "get_my_leave_requests",
                "Get leave requests for the calling employee.",
                json!({ "type": "object", "properties": { "from": { "type": "string" }, "to": { "type": "string" }, "action": { "type": "string" } }, "required": ["from", "to"], "additionalProperties": false }),
                &source_types,
            ),
            "get_my_attendance" => read(
                "get_my_attendance",
                "Get attendance for the calling employee.",
                json!({ "type": "object", "properties": { "from_date": { "type": "string" }, "to_date": { "type": "string" }, "month": { "type": "string" } }, "additionalProperties": false }),
                &source_types,
            ),
            "get_my_timesheet" => read(
                "get_my_timesheet",
                "Get timesheet entries for the calling employee.",
                json!({ "type": "object", "properties": { "from": { "type": "string" }, "to": { "type": "string" } }, "required": ["from", "to"], "additionalProperties": false }),
                &source_types,
            ),
            "apply_my_leave" => write(
                "apply_my_leave",
                "Apply leave for the calling employee.",
                json!({ "type": "object", "properties": { "leave_name": { "type": "string" }, "message": { "type": "string" }, "from_date": { "type": "string" }, "to_date": { "type": "string" }, "is_half_day": { "type": "string", "enum": ["Yes", "No"], "default": "No" }, "is_paid_or_unpaid": { "type": "string", "enum": ["paid", "unpaid"], "default": "paid" } }, "required": ["leave_name", "message", "from_date", "to_date"], "additionalProperties": false }),
                &source_types,
            ),
            "revoke_my_leave" => write(
                "revoke_my_leave",
                "Revoke leave for the calling employee.",
                json!({ "type": "object", "properties": { "leave_id": { "type": "string" }, "revoke_reason": { "type": "string" } }, "required": ["leave_id", "revoke_reason"], "additionalProperties": false }),
                &source_types,
            ),
            "list_pending_leave_approvals" => read(
                "list_pending_leave_approvals",
                "List pending leave approval requests from the calling employee's direct reports.",
                json!({ "type": "object", "properties": {}, "additionalProperties": false }),
                &source_types,
            ),
            "get_team_leave_calendar" => read(
                "get_team_leave_calendar",
                "Get leave requests within a date range for the calling employee's direct reports.",
                json!({ "type": "object", "properties": { "from": { "type": "string" }, "to": { "type": "string" } }, "required": ["from", "to"], "additionalProperties": false }),
                &source_types,
            ),
            "approve_leave_request" => write(
                "approve_leave_request",
                "Approve a direct report's leave request.",
                json!({ "type": "object", "properties": { "employee_no": { "type": "string" }, "leave_id": { "type": "string" }, "manager_message": { "type": "string" } }, "required": ["employee_no", "leave_id"], "additionalProperties": false }),
                &source_types,
            ),
            "reject_leave_request" => write(
                "reject_leave_request",
                "Reject a direct report's leave request.",
                json!({ "type": "object", "properties": { "employee_no": { "type": "string" }, "leave_id": { "type": "string" }, "manager_message": { "type": "string" } }, "required": ["employee_no", "leave_id"], "additionalProperties": false }),
                &source_types,
            ),
            _ => continue,
        };
        definitions.push(def);
    }
    definitions
}

pub async fn execute_action(
    action: &str,
    params: JsonValue,
    credentials: Option<ServiceCredential>,
    source: Option<Source>,
    actor_email: Option<String>,
) -> Result<Response> {
    // Look up the action policy to determine authorization requirements
    let policy =
        find_action_policy(action).ok_or_else(|| anyhow!("unknown Darwinbox action: {action}"))?;
    // This gate deliberately precedes trusted-source, config, credential and
    // provider access so stale allowlists cannot execute hidden actions.
    if !policy.available {
        return Err(anyhow!(
            "Darwinbox action '{action}' is not available in this connector version"
        ));
    }

    // Extract source config and actor from trusted context.
    let trusted_source = source.ok_or_else(|| {
        anyhow!("Darwinbox actions require a trusted source from connector-manager")
    })?;
    let config: DarwinboxSourceConfig = serde_json::from_value(trusted_source.config)
        .context("failed to deserialize Darwinbox source config from trusted source")?;

    // Extract the authenticated actor identity.
    let caller_email = actor_email
        .ok_or_else(|| anyhow!("Darwinbox actions require an interactive authenticated user"))?;
    if !config.is_action_participant(&caller_email) {
        return Err(anyhow!(
            "caller is not an approved Darwinbox action participant"
        ));
    }
    if policy.is_write && config.read_only {
        return Err(anyhow!(
            "action '{action}' is not allowed: source is configured as read-only"
        ));
    }
    // Decode the integration credential only after all policy-only gates.
    let client = action_client(&config, credentials)?;

    let result = (async {
        let calling_employee = if policy.audience == "self" {
            reject_identity_params(&params)?;
            Some(resolve_calling_employee(&client, &caller_email).await?)
        } else {
            None
        };

        // Audience enforcement
        match policy.audience {
            "self" => {}
            "manager" => {
                // Manager: verify user is a manager (has direct reports)
                let reports = direct_reports(&client, &caller_email, &config).await?;
                if reports.is_empty() {
                    return Err(anyhow!("caller has no direct reports"));
                }
            }
            "recruiter" => {
                // Recruiter: requires email in recruiter_emails list
                let email = &caller_email;
                let is_recruiter = config
                    .authorization
                    .recruiter_emails
                    .iter()
                    .any(|e| e.eq_ignore_ascii_case(email));
                if !is_recruiter {
                    return Err(anyhow!(
                        "action '{action}' requires recruiter authorization"
                    ));
                }
            }
            _ => {}
        }

        let result = match action {
        "get_my_profile" => {
            let employee = calling_employee.as_ref().expect("self action resolved caller");
            // Sanitized response: include only safe fields
            json!({ "employee": safe_employee_profile(employee) })
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
            // Sanitize response
            json!({ "employees": matches.into_iter().map(|e| safe_employee_profile(&e)).collect::<Vec<_>>() })
        }
        "get_my_leave_balance" => {
            let employee = calling_employee.as_ref().expect("self action resolved caller");
            let employee_no = employee_id(employee)?;
            client.fetch_leave_balance(employee_no).await?
        }
        "get_holiday_calendar" => {
            let employee = calling_employee.as_ref().expect("self action resolved caller");
            let employee_no = employee_id(employee)?;
            let year = params
                .get("year")
                .and_then(JsonValue::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| current_year(&config));
            // The action relays the raw envelope to the response sanitizer
            // (the typed `fetch_holiday_list` is for the sync module).
            client
                .post_json::<JsonValue>(
                    "/leavesactionapi/holidaylist",
                    json!({ "employee_no": employee_no, "year": year }),
                    false,
                )
                .await?
        }
        "apply_my_leave" => {
            let employee = calling_employee.as_ref().expect("self action resolved caller");
            let employee_no = employee_id(employee)?;
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
            let employee = calling_employee.as_ref().expect("self action resolved caller");
            let employee_no = employee_id(employee)?;
            client
                .revoke_leave(RevokeLeaveRequest {
                    employee_no: employee_no.to_string(),
                    leave_id: required_str(&params, "leave_id")?.to_string(),
                    revoke_reason: required_str(&params, "revoke_reason")?.to_string(),
                })
                .await?
        }
        "get_my_leave_requests" => {
            let employee = calling_employee.as_ref().expect("self action resolved caller");
            let employee_no = employee_id(employee)?;
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
            let employee = calling_employee.as_ref().expect("self action resolved caller");
            let employee_no = employee_id(employee)?;
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
            let employee = calling_employee.as_ref().expect("self action resolved caller");
            let employee_no = employee_id(employee)?;
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
            let employee = calling_employee.as_ref().expect("self action resolved caller");
            let employee_no = employee_id(employee)?;
            client
                .fetch_timesheet(
                    employee_no,
                    required_str(&params, "from")?,
                    required_str(&params, "to")?,
                )
                .await?
        }
        "list_pending_leave_approvals" => {
            let reports = direct_reports(&client, &caller_email, &config).await?;
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
            ensure_direct_report(&client, &caller_email, employee_no).await?;
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
            let reports = direct_reports(&client, &caller_email, &config).await?;
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
            let reports = direct_reports(&client, &caller_email, &config).await?;
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
            ensure_direct_report(&client, &caller_email, employee_no).await?;
            let employee = client
                .fetch_employees(Some(vec![employee_no.to_string()]), None)
                .await?
                .employee_data;
            // Sanitize response
            json!({ "employees": employee.into_iter().map(|e| safe_employee_profile(&e)).collect::<Vec<_>>() })
        }
        "add_pending_employee" => {
            client.post_json::<JsonValue>("/importapi/add", json!({ "employees": [params.get("employee").cloned().ok_or_else(|| anyhow!("employee is required"))?] }), false).await?
        }
        "activate_pending_employee" => {
            client.post_json::<JsonValue>("/importapi/activate", json!({ "user_ids": params.get("user_ids").cloned().ok_or_else(|| anyhow!("user_ids is required"))? }), false).await?
        }
        "update_employee_record" => {
            client.post_json::<JsonValue>("/importapi/update", json!({ "employees": [params.get("employee").cloned().ok_or_else(|| anyhow!("employee is required"))?] }), false).await?
        }
        "update_employment_details" => {
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
            client.post_json::<JsonValue>("/importapi/deactivate", json!({ "employees": params.get("employees").cloned().ok_or_else(|| anyhow!("employees is required"))? }), false).await?
        }
        "reactivate_employee" => {
            client.post_json::<JsonValue>("/importapi/undodeactivation", json!({ "employees": params.get("employees").cloned().ok_or_else(|| anyhow!("employees is required"))? }), false).await?
        }
        "upload_employee_document" => {
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
            client.post_json::<JsonValue>("/UpdateEmployeeDetails/employeehistory", json!({
                "from": required_str(&params, "from")?,
                "to": required_str(&params, "to")?,
                "filter_on_effective_date": params.get("filter_on_effective_date").cloned().unwrap_or(json!(0))
            }), false).await?
        }
        "fetch_report_ids" => {
            if config.authorization.allowed_report_ids.is_empty() {
                // Empty allowlist means no report actions are usable; do not
                // call the provider at all.
                json!({ "status": 1, "data": [] })
            } else {
                let response = client
                    .post_json::<JsonValue>("/reportsbuilderapi/reportids", json!({}), false)
                    .await?;
                filter_allowed_reports(response, &config.authorization.allowed_report_ids)
            }
        }
        "run_report" => {
            let report_id = required_str(&params, "report_id")?;
            if !report_is_allowlisted(&config, report_id) {
                return Err(anyhow!(
                    "report {report_id} is not allowlisted for this source"
                ));
            }
            client
                .post_json::<JsonValue>(
                    "/reportsbuilderapi/reportdatav2",
                    json!({ "report_id": report_id }),
                    false,
                )
                .await?
        }
        "list_jobs" => {
            client
                .fetch_jobs(
                    params
                        .get("job_updated_timestamp_from")
                        .and_then(JsonValue::as_str),
                )
                .await?
        }
        "get_job_detail" => {
            client
                .post_json::<JsonValue>(
                    "/JobsApiv3/Jobdetail",
                    json!({ "job_id": required_str(&params, "job_id")? }),
                    false,
                )
                .await?
        }
        "get_candidates" => {
            client
                .post_json::<JsonValue>("/JobsApiv3/BulkCandidatesData", params.clone(), false)
                .await?
        }
        "tag_candidate" => {
            client.post_json::<JsonValue>("/JobsApiv2/candidatetag", json!({ "candidate_ids": params.get("candidate_ids").cloned().ok_or_else(|| anyhow!("candidate_ids is required"))?, "tags": params.get("tags").cloned().ok_or_else(|| anyhow!("tags is required"))? }), false).await?
        }
        "reject_candidate" => {
            client.post_json::<JsonValue>("/JobsApiv3/RejectCandidate", json!({ "candidate_ids": params.get("candidate_ids").cloned().ok_or_else(|| anyhow!("candidate_ids is required"))?, "reason": params.get("reason").cloned().unwrap_or(json!("")) }), false).await?
        }
        "create_requisition" => {
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
            client.post_json::<JsonValue>("/requisitionApi/archiveRequisition", json!({ "requisition_id": required_str(&params, "requisition_id")?, "employee_id": required_str(&params, "employee_id")?, "reason": params.get("reason").and_then(JsonValue::as_str).unwrap_or("") }), false).await?
        }
            _ => return Ok::<_, anyhow::Error>(None),
        };
        Ok::<_, anyhow::Error>(Some(result))
    })
    .await
    .map_err(|error| {
        if is_not_permitted(&error) {
            anyhow!("This action is not allowed for this source: {error}")
        } else {
            error
        }
    })?;

    let Some(result) = result else {
        return Ok(ActionResponse::not_supported(action).into_response());
    };

    Ok(
        ActionResponse::success(sanitize_action_response(action, result, policy.is_write))
            .into_response(),
    )
}

/// Build a safe employee profile DTO containing only pre-approved fields.
fn safe_employee_profile(employee: &EmployeeRecord) -> serde_json::Value {
    let mut profile = serde_json::Map::new();
    let name = employee.display_name();
    profile.insert("display_name".to_string(), json!(name));
    profile.insert("employee_id".to_string(), json!(employee.employee_id));
    profile.insert(
        "company_email_id".to_string(),
        json!(employee.company_email_id),
    );
    profile.insert(
        "department_name".to_string(),
        json!(employee.department_name),
    );
    profile.insert(
        "designation_name".to_string(),
        json!(employee.designation_name),
    );
    profile.insert("office_area".to_string(), json!(employee.office_area));
    profile.insert(
        "direct_manager_employee_id".to_string(),
        json!(employee.direct_manager_employee_id),
    );
    profile.insert("employee_type".to_string(), json!(employee.employee_type));
    serde_json::Value::Object(profile)
}

fn action_client(
    config: &DarwinboxSourceConfig,
    credentials: Option<ServiceCredential>,
) -> Result<DarwinboxClient> {
    let creds = credentials.ok_or_else(|| anyhow!("Darwinbox credentials are required"))?;
    let darwinbox_creds: DarwinboxCredentials = serde_json::from_value(creds.credentials)
        .context("failed to decode Darwinbox credentials")?;
    DarwinboxClient::new(config, darwinbox_creds)
}

fn sanitize_action_response(action: &str, value: JsonValue, is_write: bool) -> JsonValue {
    if is_write {
        return json!({ "status": "submitted", "action": action });
    }

    const SAFE_SCALAR_KEYS: &[&str] = &[
        "status",
        "message",
        "success",
        "id",
        "employee_id",
        "employee_no",
        "employee_name",
        "leave_id",
        "leave_name",
        "leave_type",
        "leave_code",
        "balance",
        "available_balance",
        // Darwinbox's leave-balance envelope uses its own (misspelled) key for
        // the current balance; without it the sanitizer would drop the actual
        // balance count and leave only the leave names. Keep the correctly
        // spelled variant too in case the provider fixes the typo upstream.
        "currently_availabel_balance",
        "currently_available_balance",
        "accrued_so_far_this_year",
        "previous_balance",
        "adjustment_balance",
        "yearly_allotment",
        "taken",
        "utilized_leaves_this_year",
        "already_taken",
        "applied_unpaid",
        "system_unpaid",
        // leaveActionTakenLeaves item fields (leave requests, pending
        // approvals, team calendar). Without these the sanitizer would strip
        // every detail except the leave name.
        "applied_leave_id",
        "company_name",
        "leave_sub_category",
        "is_unpaid",
        "is_half_day",
        "is_firsthalf_secondhalf",
        "fullday_halfday_status",
        "manager_message",
        "leave_reason",
        "action_on",
        "action_by",
        "total_working_days",
        "from",
        "to",
        "from_date",
        "to_date",
        "date",
        "year",
        "month",
        "attendance_status",
        "holiday_name",
        "holiday_date",
        "display_name",
        "company_email_id",
        "department_name",
        "designation_name",
        "office_area",
        "direct_manager_employee_id",
        "employee_type",
    ];
    const SAFE_CONTAINER_KEYS: &[&str] = &[
        "employees",
        "employee",
        "data",
        "results",
        "records",
        "holidays",
        "leave_requests",
        "timesheet",
        "attendance",
    ];

    fn project_object(value: JsonValue) -> JsonValue {
        let JsonValue::Object(object) = value else {
            return JsonValue::Null;
        };
        JsonValue::Object(
            object
                .into_iter()
                .filter_map(|(key, value)| {
                    if SAFE_SCALAR_KEYS.contains(&key.as_str())
                        && !value.is_array()
                        && !value.is_object()
                    {
                        Some((key, value))
                    } else if SAFE_CONTAINER_KEYS.contains(&key.as_str()) {
                        let projected = match value {
                            JsonValue::Object(_) => project_object(value),
                            JsonValue::Array(items) => JsonValue::Array(
                                items
                                    .into_iter()
                                    .filter(|item| item.is_object())
                                    .map(project_object)
                                    .collect(),
                            ),
                            _ => JsonValue::Null,
                        };
                        Some((key, projected))
                    } else {
                        None
                    }
                })
                .collect(),
        )
    }

    project_object(value)
}

/// True when `report_id` is present in the source's report allowlist.
/// An empty allowlist allows nothing.
fn report_is_allowlisted(config: &DarwinboxSourceConfig, report_id: &str) -> bool {
    let report_id = report_id.trim();
    config
        .authorization
        .allowed_report_ids
        .iter()
        .any(|id| id.trim() == report_id)
}

/// Prune every report whose id is not in the allowlist from a provider
/// response, preserving the surrounding structure. Objects that declare a
/// `report_id` are dropped entirely when the id is not allowed; objects
/// without a `report_id` pass through unchanged.
fn filter_allowed_reports(value: JsonValue, allowed: &[String]) -> JsonValue {
    match value {
        JsonValue::Object(object) => {
            if let Some(id) = object.get("report_id").and_then(JsonValue::as_str)
                && !allowed.iter().any(|allowed| allowed.trim() == id.trim())
            {
                return JsonValue::Null;
            }
            let filtered = object
                .into_iter()
                .filter_map(|(key, child)| {
                    let child = filter_allowed_reports(child, allowed);
                    (!child.is_null()).then_some((key, child))
                })
                .collect::<serde_json::Map<_, _>>();
            if filtered.is_empty() {
                JsonValue::Null
            } else {
                JsonValue::Object(filtered)
            }
        }
        JsonValue::Array(items) => JsonValue::Array(
            items
                .into_iter()
                .filter_map(|item| {
                    let item = filter_allowed_reports(item, allowed);
                    (!item.is_null()).then_some(item)
                })
                .collect(),
        ),
        other => other,
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

fn is_not_permitted(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<DarwinboxApiError>()
        .is_some_and(|api_error| matches!(api_error, DarwinboxApiError::NotPermitted { .. }))
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
    caller_email: &str,
) -> Result<EmployeeRecord> {
    let caller_email = caller_email.trim().to_ascii_lowercase();
    let matches = client
        .fetch_employees(None, None)
        .await?
        .employee_data
        .into_iter()
        .filter(|employee| {
            employee
                .company_email_id
                .as_deref()
                .map(|email| email.trim().eq_ignore_ascii_case(&caller_email))
                .unwrap_or(false)
        })
        .collect::<Vec<_>>();
    match matches.len() {
        1 => Ok(matches.into_iter().next().expect("one employee match")),
        0 => Err(anyhow!(
            "no Darwinbox employee found for caller email {caller_email}"
        )),
        _ => Err(anyhow!(
            "multiple Darwinbox employees found for caller email {caller_email}"
        )),
    }
}

async fn direct_reports(
    client: &DarwinboxClient,
    caller_email: &str,
    config: &DarwinboxSourceConfig,
) -> Result<Vec<EmployeeRecord>> {
    let mut reports = all_direct_reports(client, caller_email).await?;
    // `max_batch_size` caps the report list handed to list/display actions;
    // it must never truncate an authorization check (see `ensure_direct_report`).
    reports.truncate(config.authorization.max_batch_size);
    Ok(reports)
}

/// All direct reports of the caller, resolved from the employee master.
/// Unbounded so authorization checks never lose scope to a display cap.
/// A report is an employee whose `direct_manager_employee_id` matches the
/// caller's `employee_id`; the caller themself is excluded even if their own
/// record self-references their manager id (guards against self-approval).
async fn all_direct_reports(
    client: &DarwinboxClient,
    caller_email: &str,
) -> Result<Vec<EmployeeRecord>> {
    let manager = resolve_calling_employee(client, caller_email).await?;
    let manager_id = employee_id(&manager)?.trim();
    let employees = client.fetch_employees(None, None).await?.employee_data;
    Ok(employees
        .into_iter()
        .filter(|employee| {
            let is_manager = employee.employee_id.as_deref().map(str::trim) == Some(manager_id);
            let reports_to_manager = employee
                .direct_manager_employee_id
                .as_deref()
                .map(str::trim)
                == Some(manager_id);
            !is_manager && reports_to_manager
        })
        .collect())
}

async fn ensure_direct_report(
    client: &DarwinboxClient,
    caller_email: &str,
    employee_no: &str,
) -> Result<()> {
    let employee_no = employee_no.trim();
    let is_report = all_direct_reports(client, caller_email)
        .await?
        .iter()
        .any(|employee| employee.employee_id.as_deref().map(str::trim) == Some(employee_no));
    if is_report {
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

#[cfg(test)]
mod tests {
    use super::{
        all_direct_reports, direct_reports, ensure_direct_report, execute_action,
        sanitize_action_response,
    };
    use crate::client::DarwinboxClient;
    use crate::credentials::DarwinboxCredentials;
    use serde_json::json;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn test_config(base_url: &str) -> crate::config::DarwinboxSourceConfig {
        serde_json::from_value(json!({
            "base_url": base_url,
            "authorization": { "participant_mode": "all", "max_batch_size": 20 }
        }))
        .unwrap()
    }

    fn test_credentials() -> DarwinboxCredentials {
        DarwinboxCredentials::Basic {
            username: "api-user".to_string(),
            password: "secret".to_string(),
            api_key: "api-key".to_string(),
            dataset_key: "dataset-key".to_string(),
        }
    }

    /// Employee master fixture: manager EMP001 plus two reports and two
    /// non-reports (one with a different manager, one with no manager id).
    fn employee_master() -> serde_json::Value {
        json!({
            "status": 1,
            "employee_data": [
                {"employee_id": "EMP001", "company_email_id": "mgr@example.com", "direct_manager_employee_id": "EMP000"},
                {"employee_id": "EMP002", "company_email_id": "r1@example.com", "direct_manager_employee_id": "EMP001"},
                {"employee_id": "EMP003", "company_email_id": "r2@example.com", "direct_manager_employee_id": " EMP001 "},
                {"employee_id": "EMP004", "company_email_id": "o1@example.com", "direct_manager_employee_id": "EMP999"},
                {"employee_id": "EMP005", "company_email_id": "o2@example.com"}
            ]
        })
    }

    async fn mock_employee_master(server: &MockServer, body: serde_json::Value) {
        Mock::given(method("POST"))
            .and(path("/masterapi/employee"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn direct_reports_are_strictly_scoped_to_the_callers_reports() {
        let server = MockServer::start().await;
        mock_employee_master(&server, employee_master()).await;
        let config = test_config(&server.uri());
        let client = DarwinboxClient::new(&config, test_credentials()).unwrap();

        let reports = direct_reports(&client, "mgr@example.com", &config)
            .await
            .unwrap();
        let ids = reports
            .iter()
            .filter_map(|e| e.employee_id.clone())
            .collect::<Vec<_>>();
        // EMP002 and EMP003 (whitespace-padded manager id still matches);
        // EMP004 (different manager) and EMP005 (no manager) are excluded.
        assert_eq!(ids, ["EMP002", "EMP003"]);
    }

    #[tokio::test]
    async fn direct_reports_excludes_the_manager_themself() {
        let server = MockServer::start().await;
        // Manager self-references their own manager id; must not appear as
        // their own direct report (guards self-approval via bad master data).
        mock_employee_master(
            &server,
            json!({
                "status": 1,
                "employee_data": [
                    {"employee_id": "EMP001", "company_email_id": "mgr@example.com", "direct_manager_employee_id": "EMP001"},
                    {"employee_id": "EMP002", "company_email_id": "r1@example.com", "direct_manager_employee_id": "EMP001"}
                ]
            }),
        )
        .await;
        let config = test_config(&server.uri());
        let client = DarwinboxClient::new(&config, test_credentials()).unwrap();

        let reports = all_direct_reports(&client, "mgr@example.com")
            .await
            .unwrap();
        let ids = reports
            .iter()
            .filter_map(|e| e.employee_id.clone())
            .collect::<Vec<_>>();
        assert_eq!(ids, ["EMP002"]);
    }

    #[tokio::test]
    async fn direct_reports_is_empty_when_the_caller_has_no_reports() {
        let server = MockServer::start().await;
        mock_employee_master(
            &server,
            json!({
                "status": 1,
                "employee_data": [
                    {"employee_id": "EMP001", "company_email_id": "mgr@example.com"},
                    {"employee_id": "EMP002", "company_email_id": "r1@example.com", "direct_manager_employee_id": "EMP999"}
                ]
            }),
        )
        .await;
        let config = test_config(&server.uri());
        let client = DarwinboxClient::new(&config, test_credentials()).unwrap();

        let reports = direct_reports(&client, "mgr@example.com", &config)
            .await
            .unwrap();
        assert!(reports.is_empty());

        // An unresolvable caller fails loudly instead of returning an empty
        // list (an empty list would open the manager gate to anyone).
        let error = direct_reports(&client, "ghost@example.com", &config)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("no Darwinbox employee"));
    }

    #[tokio::test]
    async fn direct_reports_handles_duplicate_manager_ids() {
        let server = MockServer::start().await;
        mock_employee_master(
            &server,
            json!({
                "status": 1,
                "employee_data": [
                    {"employee_id": "EMP001", "company_email_id": "mgr@example.com"},
                    {"employee_id": "EMP002", "company_email_id": "r1@example.com", "direct_manager_employee_id": "EMP001"},
                    {"employee_id": "EMP003", "company_email_id": "r2@example.com", "direct_manager_employee_id": "EMP001"}
                ]
            }),
        )
        .await;
        let config = test_config(&server.uri());
        let client = DarwinboxClient::new(&config, test_credentials()).unwrap();

        let reports = all_direct_reports(&client, "mgr@example.com")
            .await
            .unwrap();
        let ids = reports
            .iter()
            .filter_map(|e| e.employee_id.clone())
            .collect::<Vec<_>>();
        assert_eq!(ids, ["EMP002", "EMP003"]);
    }

    #[tokio::test]
    async fn ensure_direct_report_rejects_non_reports() {
        let server = MockServer::start().await;
        mock_employee_master(&server, employee_master()).await;
        let config = test_config(&server.uri());
        let client = DarwinboxClient::new(&config, test_credentials()).unwrap();

        ensure_direct_report(&client, "mgr@example.com", "EMP002")
            .await
            .unwrap();
        // Whitespace-padded id still resolves to a report.
        ensure_direct_report(&client, "mgr@example.com", "  EMP003 ")
            .await
            .unwrap();
        let error = ensure_direct_report(&client, "mgr@example.com", "EMP004")
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("EMP004 is not a direct report of the caller")
        );
        let error = ensure_direct_report(&client, "mgr@example.com", "EMP001")
            .await
            .unwrap_err();
        assert!(error.to_string().contains("not a direct report"));
    }

    #[test]
    fn report_allowlist_admits_only_configured_ids() {
        let config: crate::config::DarwinboxSourceConfig = serde_json::from_value(json!({
            "base_url": "https://example.darwinbox.in",
            "authorization": { "allowed_report_ids": ["R1", " R2 "] }
        }))
        .unwrap();
        assert!(super::report_is_allowlisted(&config, "R1"));
        assert!(super::report_is_allowlisted(&config, "R2"));
        assert!(super::report_is_allowlisted(&config, " R2 "));
        assert!(!super::report_is_allowlisted(&config, "R3"));

        let empty: crate::config::DarwinboxSourceConfig = serde_json::from_value(json!({
            "base_url": "https://example.darwinbox.in",
            "authorization": { "allowed_report_ids": [] }
        }))
        .unwrap();
        assert!(!super::report_is_allowlisted(&empty, "R1"));
    }

    #[test]
    fn report_listing_prunes_ids_not_in_the_allowlist() {
        let allowed = ["R1".to_string(), "R2".to_string()];
        let response = json!({
            "status": 1,
            "data": [
                {"report_id": "R1", "report_name": "Headcount"},
                {"report_id": "R2", "report_name": "Attrition"},
                {"report_id": "R3", "report_name": "Salaries"},
                {"report_id": "R4"}
            ]
        });
        let filtered = super::filter_allowed_reports(response.clone(), &allowed);
        let ids = filtered["data"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|item| item["report_id"].as_str())
            .collect::<Vec<_>>();
        assert_eq!(ids, ["R1", "R2"]);
        assert_eq!(filtered["status"], 1);

        // An empty allowlist removes every report.
        let filtered = super::filter_allowed_reports(response.clone(), &[]);
        assert!(filtered["data"].as_array().unwrap().is_empty());
        assert_eq!(filtered["status"], 1);

        // Nested disallowed ids are pruned; non-report content survives.
        let nested = json!({
            "status": 1,
            "results": [{"meta": {"report_id": "R9", "note": "x"}, "rows": [1, 2]}]
        });
        let filtered = super::filter_allowed_reports(nested, &allowed);
        let item = &filtered["results"][0];
        assert_eq!(item["rows"], json!([1, 2]));
        assert!(item.get("meta").is_none());

        let passthrough = super::filter_allowed_reports(json!({ "status": 1, "ok": true }), &[]);
        assert_eq!(passthrough["status"], 1);
    }

    #[test]
    fn response_projection_drops_scalar_arrays_and_sensitive_canaries() {
        let projected = sanitize_action_response(
            "get_my_leave_balance",
            json!({
                "data": [["salary", "SECRET-SALARY"]],
                "balance": 12,
                "bank_account": "SECRET-BANK",
                "results": [{"leave_id": "L1", "ctc": "SECRET-CTC"}]
            }),
            false,
        );
        let serialized = projected.to_string();
        assert!(serialized.contains("balance"));
        assert!(serialized.contains("leave_id"));
        assert!(!serialized.contains("SECRET"));
        assert!(!serialized.contains("salary"));
        assert!(!serialized.contains("bank_account"));
    }

    #[test]
    fn response_projection_keeps_leave_balance_counts() {
        // Darwinbox's real leavebalance payload: the current balance lives
        // under `currently_availabel_balance` (provider typo). It must survive
        // projection so the agent can report actual counts, not "Not specified".
        let projected = sanitize_action_response(
            "get_my_leave_balance",
            json!({
                "status": 1,
                "message": "Successfully Loaded All Leaves Balance",
                "data": [
                    {
                        "employee_name": "Dummy1 Test2",
                        "employee_no": "WWITest2",
                        "leave_id": "5fb371d07bbb2",
                        "leave_name": "Bereavement/ Compassionate Leave",
                        "currently_availabel_balance": 7,
                        "accrued_so_far_this_year": 10,
                        "previous_balance": 0,
                        "adjustment_balance": 0,
                        "yearly_allotment": 10,
                        "taken": 3,
                        "utilized_leaves_this_year": 3,
                        "is_hidden": 0,
                        "leave_code": "LPVY_14",
                        "bank_account": "SECRET-BANK"
                    },
                    {
                        "employee_no": "WWITest2",
                        "leave_name": "Unpaid",
                        "leave_code": "UVPY_2",
                        "already_taken": 0,
                        "applied_unpaid": 0,
                        "system_unpaid": 0
                    }
                ]
            }),
            false,
        );
        let item = &projected["data"][0];
        assert_eq!(item["currently_availabel_balance"], json!(7));
        assert_eq!(item["accrued_so_far_this_year"], json!(10));
        assert_eq!(item["yearly_allotment"], json!(10));
        assert_eq!(item["taken"], json!(3));
        assert_eq!(item["leave_code"], json!("LPVY_14"));
        assert_eq!(item["employee_name"], json!("Dummy1 Test2"));
        assert!(item.get("bank_account").is_none());
        assert!(item.get("is_hidden").is_none());
        // Correctly spelled variant survives too, should the provider fix the typo.
        let spelled = sanitize_action_response(
            "get_my_leave_balance",
            json!({
                "data": [{
                    "employee_no": "WWITest2",
                    "leave_name": "Privileged Leave",
                    "currently_available_balance": 12
                }]
            }),
            false,
        );
        assert_eq!(spelled["data"][0]["currently_available_balance"], json!(12));
        let unpaid = &projected["data"][1];
        assert_eq!(unpaid["already_taken"], json!(0));
        assert_eq!(unpaid["applied_unpaid"], json!(0));
        assert_eq!(unpaid["system_unpaid"], json!(0));
        assert!(unpaid.get("currently_availabel_balance").is_none());
    }

    #[test]
    fn response_projection_keeps_leave_request_fields() {
        // leaveActionTakenLeaves item shape (pending approvals / team calendar).
        // Balance, dates, reason and audit fields must survive projection so
        // the agent can answer "who is on leave and why".
        let projected = sanitize_action_response(
            "get_team_leave_calendar",
            json!({
                "status": 1,
                "message": "Successfully Loaded All Leaves",
                "data": [
                    {
                        "id": "T1",
                        "applied_leave_id": "AL1",
                        "employee_name": "Report One",
                        "company_name": "WeWork",
                        "employee_no": "EMP002",
                        "leave_name": "Privileged Leave",
                        "leave_sub_category": "Annual",
                        "from": "01-06-2026",
                        "to": "03-06-2026",
                        "is_unpaid": 0,
                        "is_half_day": 0,
                        "is_firsthalf_secondhalf": null,
                        "fullday_halfday_status": "All Full Days",
                        "message": "Family trip",
                        "manager_message": "Approved",
                        "leave_reason": "Planned leave",
                        "action_on": "01-06-2026 10:00:00",
                        "action_by": "Mgr (EMP001)",
                        "total_working_days": 3,
                        "leave_days": ["2026-06-01"],
                        "salary": "SECRET-SALARY",
                        "bank_account": "SECRET-BANK"
                    }
                ]
            }),
            false,
        );
        let item = &projected["data"][0];
        assert_eq!(item["employee_no"], json!("EMP002"));
        assert_eq!(item["leave_name"], json!("Privileged Leave"));
        assert_eq!(item["from"], json!("01-06-2026"));
        assert_eq!(item["to"], json!("03-06-2026"));
        assert_eq!(item["total_working_days"], json!(3));
        assert_eq!(item["leave_reason"], json!("Planned leave"));
        assert_eq!(item["action_by"], json!("Mgr (EMP001)"));
        assert_eq!(item["applied_leave_id"], json!("AL1"));
        assert!(!projected.to_string().contains("SECRET"));
    }

    #[test]
    fn action_definitions_expose_leave_actions() {
        let definitions = action_definitions();
        let names = definitions
            .iter()
            .map(|definition| definition.name.as_str())
            .collect::<Vec<_>>();
        for name in [
            "list_pending_leave_approvals",
            "get_team_leave_calendar",
            "approve_leave_request",
            "reject_leave_request",
        ] {
            assert!(names.contains(&name), "{name} should be exposed");
        }
        for name in [
            "regularize_my_attendance",
            "get_team_attendance_exceptions",
            "get_direct_report_profile",
            "fetch_report_ids",
            "list_jobs",
        ] {
            assert!(!names.contains(&name), "{name} should stay hidden");
        }
    }

    #[tokio::test]
    async fn unavailable_action_is_rejected_before_trusted_context() {
        let error = execute_action("regularize_my_attendance", json!({}), None, None, None)
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("not available in this connector version")
        );
    }
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
        required_scopes: None,
        source_types: source_types.to_vec(),
        admin_only,
        hidden: false,
    }
}
