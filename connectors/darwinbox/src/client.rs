use anyhow::{Context, Result, anyhow};
use reqwest::{Client, StatusCode, header};
use serde::de::DeserializeOwned;
use serde_json::{Value as JsonValue, json};
use url::Url;

use crate::auth::{add_api_key_and_dataset, apply_basic_auth, fetch_token};
use crate::config::DarwinboxSourceConfig;
use crate::credentials::DarwinboxCredentials;
use crate::models::EmployeeDataResponse;

/// Darwinbox API call outcome, split so sync/action callers can tolerate
/// provider-side denials without treating them as transient failures.
#[derive(Debug, Clone)]
pub enum DarwinboxApiError {
    /// Provider denied the request (HTTP 401/403 or other client denial). The
    /// dataset/API key does not grant this endpoint; retrying will not help.
    NotPermitted { path: &'static str, status: u16 },
    /// Rate limited (429) or server-side failure (5xx); safe to retry.
    Retryable { status: u16 },
    /// Other non-success HTTP status (e.g. 400, 404).
    Other { status: u16 },
    /// Transport-level failure (DNS, connect, timeout).
    Transport(String),
}

impl std::fmt::Display for DarwinboxApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotPermitted { path, status } => write!(
                f,
                "Darwinbox denied {} access (HTTP {}); grant the endpoint via the dataset/API key configuration",
                capability_for_endpoint(path),
                status
            ),
            Self::Retryable { status } => {
                write!(f, "Darwinbox API returned retryable HTTP {status}")
            }
            Self::Other { status } => write!(f, "Darwinbox API returned HTTP {status}"),
            Self::Transport(message) => write!(f, "Darwinbox API request failed: {message}"),
        }
    }
}

impl std::error::Error for DarwinboxApiError {}

#[derive(Debug, Clone)]
pub struct ApplyLeaveRequest {
    pub employee_no: String,
    pub leave_name: String,
    pub message: String,
    pub from_date: String,
    pub to_date: String,
    pub is_half_day: String,
    pub is_paid_or_unpaid: String,
}

#[derive(Debug, Clone)]
pub struct RevokeLeaveRequest {
    pub employee_no: String,
    pub leave_id: String,
    pub revoke_reason: String,
}

#[derive(Debug, Clone)]
pub struct LeaveRequestsRequest {
    pub employee_nos: Vec<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub action: String,
}

#[derive(Debug, Clone, Copy)]
pub enum LeaveDecision {
    Approve,
    Reject,
}

impl LeaveDecision {
    fn as_darwinbox_action(self) -> &'static str {
        match self {
            Self::Approve => "approve",
            Self::Reject => "reject",
        }
    }
}

#[derive(Debug, Clone)]
pub struct LeaveDecisionRequest {
    pub employee_no: String,
    pub leave_id: String,
    pub decision: LeaveDecision,
    pub manager_message: Option<String>,
}

#[derive(Debug, Clone)]
pub struct MonthlyAttendanceRequest {
    pub employee_nos: Vec<String>,
    pub from_date: Option<String>,
    pub to_date: Option<String>,
    pub month: Option<String>,
}

#[derive(Clone)]
pub struct DarwinboxClient {
    http: Client,
    base_url: String,
    credentials: DarwinboxCredentials,
}

impl DarwinboxClient {
    pub fn new(config: &DarwinboxSourceConfig, credentials: DarwinboxCredentials) -> Result<Self> {
        let mut url = Url::parse(config.base_url.trim()).context("invalid Darwinbox base_url")?;
        let is_loopback = url.host_str().is_some_and(|host| {
            host.eq_ignore_ascii_case("localhost")
                || host
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|ip| ip.is_loopback())
        });
        if url.scheme() != "https" && !(url.scheme() == "http" && is_loopback) {
            return Err(anyhow!("Darwinbox base_url must use HTTPS for security"));
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(anyhow!("Darwinbox base_url must not include credentials"));
        }
        url.set_fragment(None);
        if !url.path().ends_with('/') {
            let path = format!("{}/", url.path());
            url.set_path(&path);
        }
        let http = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .context("failed to build Darwinbox HTTP client")?;
        Ok(Self {
            http,
            base_url: url.to_string(),
            credentials,
        })
    }

    pub async fn validate_connection(&self) -> Result<()> {
        let _ = self.fetch_employees(None, None).await?;
        Ok(())
    }

    pub async fn fetch_employees(
        &self,
        employee_ids: Option<Vec<String>>,
        last_modified: Option<&str>,
    ) -> std::result::Result<EmployeeDataResponse, DarwinboxApiError> {
        let mut body = json!({});
        if let Some(ids) = employee_ids {
            body["employee_ids"] = json!(ids);
        }
        if let Some(ts) = last_modified {
            body["last_modified"] = json!(ts);
        }
        self.post_json("/masterapi/employee", body, true).await
    }

    pub async fn fetch_deleted_employees(
        &self,
        last_modified: Option<&str>,
    ) -> std::result::Result<JsonValue, DarwinboxApiError> {
        let mut body = json!({});
        if let Some(ts) = last_modified {
            body["last_modified"] = json!(ts);
        }
        self.post_json("/UpdateEmployeeDetails/getDeletedEmployees", body, false)
            .await
    }

    pub async fn fetch_org_master(
        &self,
        path: &str,
    ) -> std::result::Result<JsonValue, DarwinboxApiError> {
        self.post_json(path, json!({}), false).await
    }

    pub async fn fetch_position_master(
        &self,
        last_modified: Option<&str>,
    ) -> std::result::Result<JsonValue, DarwinboxApiError> {
        let mut body = json!({ "status": 0, "need_to_hire": 2 });
        if let Some(ts) = last_modified {
            body["last_modified"] = json!(ts);
        }
        self.post_json("/orgmasterapi/getpositionMaster", body, false)
            .await
    }

    pub async fn fetch_holiday_list(
        &self,
        employee_no: &str,
        year: &str,
    ) -> std::result::Result<JsonValue, DarwinboxApiError> {
        self.post_json(
            "/leavesactionapi/holidaylist",
            json!({ "employee_no": employee_no, "year": year }),
            false,
        )
        .await
    }

    pub async fn fetch_leave_balance(
        &self,
        employee_no: &str,
    ) -> std::result::Result<JsonValue, DarwinboxApiError> {
        self.post_json(
            "/leavesactionapi/leavebalance",
            json!({ "employee_nos": [employee_no], "ignore_rounding": "1" }),
            false,
        )
        .await
    }

    pub async fn apply_leave(
        &self,
        request: ApplyLeaveRequest,
    ) -> std::result::Result<JsonValue, DarwinboxApiError> {
        self.post_json(
            "/leavesactionapi/importleave",
            json!({
                "data": [{
                    "employee_no": request.employee_no,
                    "leave_name": request.leave_name,
                    "message": request.message,
                    "from_date": request.from_date,
                    "to_date": request.to_date,
                    "is_half_day": request.is_half_day,
                    "is_paid_or_unpaid": request.is_paid_or_unpaid,
                    "revoke_leave": "No"
                }]
            }),
            false,
        )
        .await
    }

    pub async fn revoke_leave(
        &self,
        request: RevokeLeaveRequest,
    ) -> std::result::Result<JsonValue, DarwinboxApiError> {
        self.post_json(
            "/leavesactionapi/importleave",
            json!({
                "data": [{
                    "employee_no": request.employee_no,
                    "leave_id": request.leave_id,
                    "revoke_leave": "Yes",
                    "revoke_reason": request.revoke_reason
                }]
            }),
            false,
        )
        .await
    }

    pub async fn fetch_leave_requests(
        &self,
        request: LeaveRequestsRequest,
    ) -> std::result::Result<JsonValue, DarwinboxApiError> {
        let mut body = json!({
            "employee_no": request.employee_nos,
            "action": request.action,
        });
        if let Some(from) = request.from {
            body["from"] = json!(from);
        }
        if let Some(to) = request.to {
            body["to"] = json!(to);
        }
        self.post_json("/leavesactionapi/leaveActionTakenLeaves", body, false)
            .await
    }

    pub async fn take_leave_decision(
        &self,
        request: LeaveDecisionRequest,
    ) -> std::result::Result<JsonValue, DarwinboxApiError> {
        self.post_json(
            "/leavesactionapi/leaveaction",
            json!({
                "employee_no": request.employee_no,
                "leave_id": request.leave_id,
                "action": request.decision.as_darwinbox_action(),
                "manager_message": request.manager_message.unwrap_or_default()
            }),
            false,
        )
        .await
    }

    pub async fn fetch_monthly_attendance(
        &self,
        request: MonthlyAttendanceRequest,
    ) -> std::result::Result<JsonValue, DarwinboxApiError> {
        let mut body = json!({ "emp_number_list": request.employee_nos });
        if let Some(from_date) = request.from_date {
            body["from_date"] = json!(from_date);
        }
        if let Some(to_date) = request.to_date {
            body["to_date"] = json!(to_date);
        }
        if let Some(month) = request.month {
            body["month"] = json!(month);
        }
        self.post_json("/AttendanceDataApi/monthly", body, false)
            .await
    }

    pub async fn regularize_attendance(
        &self,
        employee_no: &str,
        mut attendance: JsonValue,
    ) -> std::result::Result<JsonValue, DarwinboxApiError> {
        let object = attendance.as_object_mut().ok_or_else(|| {
            DarwinboxApiError::Transport("attendance must be an object".to_string())
        })?;
        object.insert("employee_no".to_string(), json!(employee_no));
        self.post_json("/attendanceDataApi/backdatedattendance", attendance, false)
            .await
    }

    pub async fn fetch_timesheet(
        &self,
        employee_no: &str,
        from: &str,
        to: &str,
    ) -> std::result::Result<JsonValue, DarwinboxApiError> {
        self.post_json(
            "/attendanceDataApi/timesheetdatewise",
            json!({
                "employee_no": [employee_no],
                "from": from,
                "to": to
            }),
            false,
        )
        .await
    }

    pub async fn fetch_daily_attendance_roster(
        &self,
        employee_nos: Vec<String>,
        from_date: &str,
        to_date: &str,
    ) -> std::result::Result<JsonValue, DarwinboxApiError> {
        self.post_json(
            "/attendanceDataApi/DailyAttendanceRoster",
            json!({
                "emp_number_list": employee_nos,
                "from_date": from_date,
                "to_date": to_date
            }),
            false,
        )
        .await
    }

    pub async fn fetch_jobs(
        &self,
        updated_from: Option<&str>,
    ) -> std::result::Result<JsonValue, DarwinboxApiError> {
        let mut body = json!({});
        if let Some(ts) = updated_from {
            body["job_updated_timestamp_from"] = json!(ts);
        }
        self.post_json("/JobsApiv3/Joblist", body, false).await
    }

    pub async fn post_json<T: DeserializeOwned>(
        &self,
        path: &str,
        body: JsonValue,
        include_dataset_key: bool,
    ) -> std::result::Result<T, DarwinboxApiError> {
        // Use Url::join for safe URL construction
        let base = Url::parse(&self.base_url)
            .map_err(|error| DarwinboxApiError::Transport(format!("invalid base_url: {error}")))?;
        let url = base
            .join(path.trim_start_matches('/'))
            .map_err(|error| DarwinboxApiError::Transport(format!("failed to join URL: {error}")))?
            .to_string();
        let body = add_api_key_and_dataset(body, &self.credentials, include_dataset_key);
        let mut request = self
            .http
            .post(url)
            .header("Content-Type", "application/json")
            .json(&body);

        request = apply_basic_auth(request, &self.credentials);
        if !matches!(self.credentials, DarwinboxCredentials::Basic { .. }) {
            let token = fetch_token(&self.http, &self.base_url, &self.credentials)
                .await
                .map_err(|error| DarwinboxApiError::Transport(error.to_string()))?
                .ok_or_else(|| {
                    DarwinboxApiError::Transport("token auth did not return a token".to_string())
                })?;
            // Darwinbox OAuth 2.0 expects the access token as a Bearer token;
            // sending it in a custom TOKEN header makes every business API
            // return 401 "Invalid Credentials".
            request = request.header(
                header::AUTHORIZATION,
                format!("Bearer {}", token.access_token),
            );
        }

        let mut last_error = None;
        for attempt in 0..3 {
            let response = request
                .try_clone()
                .ok_or_else(|| {
                    DarwinboxApiError::Transport(
                        "failed to clone Darwinbox API request".to_string(),
                    )
                })?
                .send()
                .await
                .map_err(|error| DarwinboxApiError::Transport(error.to_string()))?;
            let status = response.status();
            if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
                // Never deserialize or log the denial body: Darwinbox error
                // envelopes can echo credentials, datasets, or employee data.
                return Err(DarwinboxApiError::NotPermitted {
                    path: capability_for_endpoint(path),
                    status: status.as_u16(),
                });
            }
            if status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
                last_error = Some(DarwinboxApiError::Retryable {
                    status: status.as_u16(),
                });
                tokio::time::sleep(std::time::Duration::from_millis(250 * (attempt + 1))).await;
                continue;
            }
            if !status.is_success() {
                return Err(DarwinboxApiError::Other {
                    status: status.as_u16(),
                });
            }

            return response
                .json::<T>()
                .await
                .map_err(|error| DarwinboxApiError::Transport(error.to_string()));
        }

        Err(last_error.unwrap_or(DarwinboxApiError::Transport(
            "Darwinbox API request failed".to_string(),
        )))
    }
}

fn capability_for_endpoint(path: &str) -> &'static str {
    match path {
        "/masterapi/employee" => "People directory/caller resolution",
        "/leavesactionapi/leavebalance" => "leave balance",
        "/leavesactionapi/holidaylist" => "holiday calendar",
        "/leavesactionapi/leaveActionTakenLeaves" => "leave requests",
        "/AttendanceDataApi/monthly" => "attendance",
        "/attendanceDataApi/timesheetdatewise" => "timesheet",
        "/leavesactionapi/importleave" => "leave changes",
        _ => "selected Darwinbox endpoint",
    }
}
