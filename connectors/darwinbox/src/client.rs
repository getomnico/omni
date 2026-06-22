use anyhow::{anyhow, Context, Result};
use reqwest::{Client, StatusCode};
use serde::de::DeserializeOwned;
use serde_json::{json, Value as JsonValue};

use crate::auth::{add_api_key_and_dataset, apply_basic_auth, fetch_token};
use crate::config::DarwinboxSourceConfig;
use crate::credentials::DarwinboxCredentials;
use crate::models::EmployeeDataResponse;

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
        let base_url = config.base_url.trim_end_matches('/').to_string();
        if base_url.is_empty() {
            return Err(anyhow!("Darwinbox base_url is required"));
        }
        Ok(Self {
            http: Client::new(),
            base_url,
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
    ) -> Result<EmployeeDataResponse> {
        let mut body = json!({});
        if let Some(ids) = employee_ids {
            body["employee_ids"] = json!(ids);
        }
        if let Some(ts) = last_modified {
            body["last_modified"] = json!(ts);
        }
        self.post_json("/masterapi/employee", body, true).await
    }

    pub async fn fetch_deleted_employees(&self, last_modified: Option<&str>) -> Result<JsonValue> {
        let mut body = json!({});
        if let Some(ts) = last_modified {
            body["last_modified"] = json!(ts);
        }
        self.post_json("/UpdateEmployeeDetails/getDeletedEmployees", body, false)
            .await
    }

    pub async fn fetch_org_master(&self, path: &str) -> Result<JsonValue> {
        self.post_json(path, json!({}), false).await
    }

    pub async fn fetch_position_master(&self, last_modified: Option<&str>) -> Result<JsonValue> {
        let mut body = json!({ "status": 0, "need_to_hire": 2 });
        if let Some(ts) = last_modified {
            body["last_modified"] = json!(ts);
        }
        self.post_json("/orgmasterapi/getpositionMaster", body, false)
            .await
    }

    pub async fn fetch_holiday_list(&self, employee_no: &str, year: &str) -> Result<JsonValue> {
        self.post_json(
            "/leavesactionapi/holidaylist",
            json!({ "employee_no": employee_no, "year": year }),
            false,
        )
        .await
    }

    pub async fn fetch_leave_balance(&self, employee_no: &str) -> Result<JsonValue> {
        self.post_json(
            "/leavesactionapi/leavebalance",
            json!({ "employee_nos": [employee_no], "ignore_rounding": "1" }),
            false,
        )
        .await
    }

    pub async fn apply_leave(&self, request: ApplyLeaveRequest) -> Result<JsonValue> {
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

    pub async fn revoke_leave(&self, request: RevokeLeaveRequest) -> Result<JsonValue> {
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

    pub async fn fetch_leave_requests(&self, request: LeaveRequestsRequest) -> Result<JsonValue> {
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

    pub async fn take_leave_decision(&self, request: LeaveDecisionRequest) -> Result<JsonValue> {
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
    ) -> Result<JsonValue> {
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
    ) -> Result<JsonValue> {
        let object = attendance
            .as_object_mut()
            .ok_or_else(|| anyhow!("attendance must be an object"))?;
        object.insert("employee_no".to_string(), json!(employee_no));
        self.post_json("/attendanceDataApi/backdatedattendance", attendance, false)
            .await
    }

    pub async fn fetch_timesheet(
        &self,
        employee_no: &str,
        from: &str,
        to: &str,
    ) -> Result<JsonValue> {
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
    ) -> Result<JsonValue> {
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

    pub async fn fetch_jobs(&self, updated_from: Option<&str>) -> Result<JsonValue> {
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
    ) -> Result<T> {
        let url = format!("{}{}", self.base_url, path);
        let body = add_api_key_and_dataset(body, &self.credentials, include_dataset_key);
        let mut request = self
            .http
            .post(url)
            .header("Content-Type", "application/json")
            .json(&body);

        request = apply_basic_auth(request, &self.credentials);
        if !matches!(self.credentials, DarwinboxCredentials::Basic { .. }) {
            let token = fetch_token(&self.http, &self.base_url, &self.credentials)
                .await?
                .ok_or_else(|| anyhow!("token auth did not return a token"))?;
            request = request.header("TOKEN", token.access_token);
        }

        let mut last_error = None;
        for attempt in 0..3 {
            let response = request
                .try_clone()
                .ok_or_else(|| anyhow!("failed to clone Darwinbox API request"))?
                .send()
                .await
                .with_context(|| format!("failed to call Darwinbox API {path}"))?;
            let status = response.status();
            if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
                let body = response.text().await.unwrap_or_default();
                return Err(anyhow!(
                    "Darwinbox authentication/authorization failed ({status}): {body}"
                ));
            }
            if status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
                let body = response.text().await.unwrap_or_default();
                last_error = Some(anyhow!(
                    "Darwinbox API returned retryable HTTP {status}: {body}"
                ));
                tokio::time::sleep(std::time::Duration::from_millis(250 * (attempt + 1))).await;
                continue;
            }
            if !status.is_success() {
                let body = response.text().await.unwrap_or_default();
                return Err(anyhow!("Darwinbox API returned HTTP {status}: {body}"));
            }

            return response
                .json::<T>()
                .await
                .with_context(|| format!("failed to parse Darwinbox API response for {path}"));
        }

        Err(last_error.unwrap_or_else(|| anyhow!("Darwinbox API request failed")))
    }
}
