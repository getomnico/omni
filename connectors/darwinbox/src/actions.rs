use anyhow::{Context, Result, anyhow};
use axum::response::Response;
use chrono::{Datelike, Utc};
use omni_connector_sdk::{
    ActionDefinition, ActionMode, ActionResponse, ServiceCredential, Source, SourceType,
};
use serde_json::{Value as JsonValue, json};

use crate::client::{
    ApplyLeaveRequest, DarwinboxClient, LeaveDecision, LeaveDecisionRequest, LeaveRequestsRequest,
    MonthlyAttendanceRequest, RevokeLeaveRequest,
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
        // Manager workflows — unavailable until authorization and typing are reviewed
        ActionPolicy {
            name: "list_pending_leave_approvals",
            module: "manager_workflows",
            mode: ActionMode::Read,
            audience: "manager",
            is_write: false,
            available: false,
        },
        ActionPolicy {
            name: "get_team_leave_calendar",
            module: "manager_workflows",
            mode: ActionMode::Read,
            audience: "manager",
            is_write: false,
            available: false,
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
        // Manager writes
        ActionPolicy {
            name: "approve_leave_request",
            module: "manager_workflows",
            mode: ActionMode::Write,
            audience: "manager",
            is_write: true,
            available: false,
        },
        ActionPolicy {
            name: "reject_leave_request",
            module: "manager_workflows",
            mode: ActionMode::Write,
            audience: "manager",
            is_write: true,
            available: false,
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

    if !config.authorization.actions_enabled {
        return Err(anyhow!("Darwinbox actions are disabled for this source"));
    }

    // Extract the authenticated actor identity.
    let caller_email = actor_email
        .ok_or_else(|| anyhow!("Darwinbox actions require an interactive authenticated user"))?;
    if !config.is_action_participant(&caller_email) {
        return Err(anyhow!(
            "caller is not an approved Darwinbox action participant"
        ));
    }
    if !config
        .authorization
        .allowed_actions
        .iter()
        .any(|allowed| allowed == action)
    {
        return Err(anyhow!(
            "action '{action}' is not allowlisted for this source"
        ));
    }
    if policy.is_write && config.read_only {
        return Err(anyhow!(
            "action '{action}' is not allowed: source is configured as read-only"
        ));
    }
    // Decode the integration credential only after all policy-only gates.
    let client = action_client(&config, credentials)?;

    // Module enforcement: check that the action's module is enabled
    match policy.module {
        "employee_self_service" => ensure_enabled(
            config.action_modules.employee_self_service,
            "employee_self_service",
        )?,
        "manager_workflows" => {
            ensure_enabled(config.action_modules.manager_workflows, "manager_workflows")?
        }
        "hr_operations" => ensure_enabled(config.action_modules.hr_operations, "hr_operations")?,
        "ats" => ensure_enabled(config.action_modules.ats, "ats")?,
        "reports" => ensure_enabled(config.action_modules.reports, "reports")?,
        _ => {} // no module requirement
    }

    let calling_employee = if policy.audience == "self" {
        reject_identity_params(&params)?;
        Some(resolve_calling_employee(&client, &caller_email).await?)
    } else {
        None
    };

    // 3. Audience enforcement
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
                .filter(|employee| config.is_employee_in_scope(employee))
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
            client.fetch_holiday_list(employee_no, &year).await?
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
            ensure_direct_report(&client, &caller_email, &config, employee_no).await?;
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
            ensure_direct_report(&client, &caller_email, &config, employee_no).await?;
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
            client
                .post_json::<JsonValue>("/reportsbuilderapi/reportids", json!({}), false)
                .await?
        }
        "run_report" => {
            client
                .post_json::<JsonValue>(
                    "/reportsbuilderapi/reportdatav2",
                    json!({ "report_id": required_str(&params, "report_id")? }),
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
        _ => return Ok(ActionResponse::not_supported(action).into_response()),
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
        "leave_id",
        "leave_name",
        "leave_type",
        "balance",
        "available_balance",
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
    caller_email: &str,
) -> Result<EmployeeRecord> {
    let caller_email = caller_email.to_ascii_lowercase();
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
    let manager = resolve_calling_employee(client, caller_email).await?;
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
        .take(config.authorization.max_batch_size)
        .collect())
}

async fn ensure_direct_report(
    client: &DarwinboxClient,
    caller_email: &str,
    config: &DarwinboxSourceConfig,
    employee_no: &str,
) -> Result<()> {
    if direct_reports(client, caller_email, config)
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

#[cfg(test)]
mod tests {
    use super::{execute_action, sanitize_action_response};
    use serde_json::json;

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
