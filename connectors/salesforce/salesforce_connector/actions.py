"""Action definitions and typed execution for the Salesforce connector."""

from __future__ import annotations

import logging
from collections.abc import Mapping
from dataclasses import dataclass

from fastapi.responses import JSONResponse
from omni_connector import ActionDefinition, ActionResponse

from .client import (
    AuthenticationError,
    ForbiddenError,
    NotFoundError,
    SalesforceClient,
    SalesforceClientError,
)
from .config import SalesforceObjectConfig, config_for
from .models import _as_int, _as_str

logger = logging.getLogger(__name__)

MAX_RESULTS = 50
DEFAULT_RESULTS = 10

SUPPORTED_ACTION_OBJECTS = ("Account", "Contact", "Opportunity", "Lead", "Case", "Task")


def _search_fields(config: SalesforceObjectConfig) -> tuple[str, ...]:
    """Fields matched by the free-text search in find_records."""
    if config.name == "Account":
        return ("Name",)
    if config.name == "Contact":
        return ("Name", "Email")
    if config.name == "Opportunity":
        return ("Name",)
    if config.name == "Lead":
        return ("Name", "Company", "Email")
    if config.name == "Case":
        return ("CaseNumber", "Subject")
    if config.name == "Task":
        return ("Subject",)
    return ()


def _object_schema(properties: dict[str, object], required: list[str]) -> dict[str, object]:
    return {"type": "object", "properties": properties, "required": required}


def _case_type_property() -> dict[str, object]:
    return {
        "type": "string",
        "description": "Case type, e.g. 'Mechanical', 'Electrical'",
    }


def _status_property(description: str) -> dict[str, object]:
    return {"type": "string", "description": description}


ACTION_DEFINITIONS: tuple[ActionDefinition, ...] = (
    ActionDefinition(
        name="find_records",
        description=(
            "Search Salesforce records (accounts, contacts, opportunities, leads, "
            "cases, tasks) by name or subject and return matching records"
        ),
        input_schema=_object_schema(
            {
                "object_type": {
                    "type": "string",
                    "enum": list(SUPPORTED_ACTION_OBJECTS),
                    "description": "Salesforce object type to search",
                },
                "query": {
                    "type": "string",
                    "description": "Free-text query matched against name/subject fields",
                },
                "limit": {
                    "type": "integer",
                    "description": f"Max results (default {DEFAULT_RESULTS}, max {MAX_RESULTS})",
                },
            },
            ["object_type"],
        ),
        mode="read",
        source_types=["salesforce"],
    ),
    ActionDefinition(
        name="get_case",
        description="Fetch a Salesforce case by its id, including status, priority, and account",
        input_schema=_object_schema(
            {"case_id": {"type": "string", "description": "Salesforce Case Id"}},
            ["case_id"],
        ),
        mode="read",
        source_types=["salesforce"],
    ),
    ActionDefinition(
        name="create_case",
        description="Create a new Salesforce case",
        input_schema=_object_schema(
            {
                "subject": {"type": "string", "description": "Case subject (required)"},
                "description": {"type": "string", "description": "Case description"},
                "status": _status_property("Case status, e.g. 'New', 'Working', 'Closed'"),
                "priority": _status_property("Case priority, e.g. 'High', 'Medium', 'Low'"),
                "type": _case_type_property(),
                "origin": {"type": "string", "description": "Case origin, e.g. 'Web', 'Email'"},
                "account_id": {"type": "string", "description": "Related Account Id"},
                "contact_id": {"type": "string", "description": "Related Contact Id"},
            },
            ["subject"],
        ),
        mode="write",
        source_types=["salesforce"],
    ),
    ActionDefinition(
        name="update_case_status",
        description="Update the status (and optionally priority) of a Salesforce case",
        input_schema=_object_schema(
            {
                "case_id": {"type": "string", "description": "Salesforce Case Id"},
                "status": _status_property("New case status, e.g. 'Working', 'Closed'"),
                "priority": _status_property("New case priority, e.g. 'High', 'Medium', 'Low'"),
            },
            ["case_id", "status"],
        ),
        mode="write",
        source_types=["salesforce"],
    ),
    ActionDefinition(
        name="create_task",
        description="Create a new Salesforce task",
        input_schema=_object_schema(
            {
                "subject": {"type": "string", "description": "Task subject (required)"},
                "description": {"type": "string", "description": "Task description"},
                "status": _status_property("Task status, e.g. 'Not Started', 'Completed'"),
                "priority": _status_property("Task priority, e.g. 'High', 'Normal', 'Low'"),
                "activity_date": {
                    "type": "string",
                    "description": "Due date in YYYY-MM-DD format",
                },
                "who_id": {"type": "string", "description": "Related contact/lead Id"},
                "what_id": {"type": "string", "description": "Related account/opportunity/case Id"},
            },
            ["subject"],
        ),
        mode="write",
        source_types=["salesforce"],
    ),
    ActionDefinition(
        name="update_task_status",
        description="Update the status of a Salesforce task",
        input_schema=_object_schema(
            {
                "task_id": {"type": "string", "description": "Salesforce Task Id"},
                "status": _status_property("New task status, e.g. 'In Progress', 'Completed'"),
            },
            ["task_id", "status"],
        ),
        mode="write",
        source_types=["salesforce"],
    ),
)


@dataclass(frozen=True)
class FindRecordsParams:
    object_type: str
    query: str | None = None
    limit: int = DEFAULT_RESULTS

    @classmethod
    def from_mapping(cls, raw: Mapping[str, object]) -> FindRecordsParams:
        object_type = _as_str(raw.get("object_type"))
        if object_type is None:
            raise ValueError("object_type is required")
        limit = _as_int(raw.get("limit")) or DEFAULT_RESULTS
        return cls(
            object_type=object_type,
            query=_as_str(raw.get("query")),
            limit=max(1, min(limit, MAX_RESULTS)),
        )


@dataclass(frozen=True)
class CaseIdParams:
    case_id: str

    @classmethod
    def from_mapping(cls, raw: Mapping[str, object]) -> CaseIdParams:
        case_id = _as_str(raw.get("case_id"))
        if case_id is None:
            raise ValueError("case_id is required")
        return cls(case_id=case_id)


@dataclass(frozen=True)
class CreateCaseParams:
    subject: str
    description: str | None = None
    status: str | None = None
    priority: str | None = None
    type: str | None = None
    origin: str | None = None
    account_id: str | None = None
    contact_id: str | None = None

    @classmethod
    def from_mapping(cls, raw: Mapping[str, object]) -> CreateCaseParams:
        subject = _as_str(raw.get("subject"))
        if subject is None:
            raise ValueError("subject is required")
        return cls(
            subject=subject,
            description=_as_str(raw.get("description")),
            status=_as_str(raw.get("status")),
            priority=_as_str(raw.get("priority")),
            type=_as_str(raw.get("type")),
            origin=_as_str(raw.get("origin")),
            account_id=_as_str(raw.get("account_id")),
            contact_id=_as_str(raw.get("contact_id")),
        )


@dataclass(frozen=True)
class UpdateCaseParams:
    case_id: str
    status: str
    priority: str | None = None

    @classmethod
    def from_mapping(cls, raw: Mapping[str, object]) -> UpdateCaseParams:
        case_id = _as_str(raw.get("case_id"))
        status = _as_str(raw.get("status"))
        if case_id is None:
            raise ValueError("case_id is required")
        if status is None:
            raise ValueError("status is required")
        return cls(case_id=case_id, status=status, priority=_as_str(raw.get("priority")))


@dataclass(frozen=True)
class CreateTaskParams:
    subject: str
    description: str | None = None
    status: str | None = None
    priority: str | None = None
    activity_date: str | None = None
    who_id: str | None = None
    what_id: str | None = None

    @classmethod
    def from_mapping(cls, raw: Mapping[str, object]) -> CreateTaskParams:
        subject = _as_str(raw.get("subject"))
        if subject is None:
            raise ValueError("subject is required")
        return cls(
            subject=subject,
            description=_as_str(raw.get("description")),
            status=_as_str(raw.get("status")),
            priority=_as_str(raw.get("priority")),
            activity_date=_as_str(raw.get("activity_date")),
            who_id=_as_str(raw.get("who_id")),
            what_id=_as_str(raw.get("what_id")),
        )


@dataclass(frozen=True)
class UpdateTaskParams:
    task_id: str
    status: str

    @classmethod
    def from_mapping(cls, raw: Mapping[str, object]) -> UpdateTaskParams:
        task_id = _as_str(raw.get("task_id"))
        status = _as_str(raw.get("status"))
        if task_id is None:
            raise ValueError("task_id is required")
        if status is None:
            raise ValueError("status is required")
        return cls(task_id=task_id, status=status)


def _client_from_credentials(
    credentials: Mapping[str, object],
) -> SalesforceClient:
    access_token = _as_str(credentials.get("access_token"))
    instance_url = _as_str(credentials.get("instance_url"))
    if access_token is None:
        raise ValueError("Missing access_token in credentials")
    if instance_url is None:
        raise ValueError("Missing instance_url in credentials")
    return SalesforceClient(access_token=access_token, instance_url=instance_url)


def _summary(object_type: str, raw: Mapping[str, object]) -> dict[str, str]:
    """Small stable summary of a record for action results."""
    summary: dict[str, str] = {"id": str(raw.get("Id", ""))}
    for key, field_name in (
        ("name", "Name"),
        ("subject", "Subject"),
        ("case_number", "CaseNumber"),
        ("status", "Status"),
        ("stage", "StageName"),
        ("priority", "Priority"),
    ):
        value = raw.get(field_name)
        if isinstance(value, str) and value:
            summary[key] = value
    return summary


async def _find_records(
    credentials: Mapping[str, object], raw_params: Mapping[str, object]
) -> JSONResponse:
    params = FindRecordsParams.from_mapping(raw_params)
    config = config_for(params.object_type)
    if config is None:
        return ActionResponse.failure(f"Unsupported object type: {params.object_type}").to_response(
            status_code=400
        )

    search_fields = _search_fields(config)
    if not search_fields:
        return ActionResponse.failure(f"No searchable fields for {params.object_type}").to_response(
            status_code=400
        )

    soql = f"SELECT {', '.join(config.all_fields())} FROM {config.name}"
    if params.query:
        escaped = params.query.replace("\\", "\\\\").replace("'", "\\'")
        where = " OR ".join(f"{f} LIKE '%{escaped}%'" for f in search_fields)
        soql += f" WHERE {where}"
    soql += f" ORDER BY Id LIMIT {params.limit}"

    client = _client_from_credentials(credentials)
    try:
        result = await client.query(soql)
        return ActionResponse.success(
            {
                "object_type": config.name,
                "records": [_summary(config.name, record) for record in result.records],
            }
        ).to_response()
    except AuthenticationError as e:
        return ActionResponse.failure(f"Authentication failed: {e}").to_response(status_code=401)
    except SalesforceClientError as e:
        return ActionResponse.failure(f"Salesforce API error: {e}").to_response(status_code=502)


async def _get_case(
    credentials: Mapping[str, object], raw_params: Mapping[str, object]
) -> JSONResponse:
    params = CaseIdParams.from_mapping(raw_params)
    client = _client_from_credentials(credentials)
    try:
        raw = await client.get_record(
            "Case",
            params.case_id,
            ("Id", "CaseNumber", "Subject", "Description", "Status", "Priority"),
        )
        return ActionResponse.success(dict(raw)).to_response()
    except NotFoundError:
        return ActionResponse.failure(f"Case not found: {params.case_id}").to_response(
            status_code=404
        )
    except AuthenticationError as e:
        return ActionResponse.failure(f"Authentication failed: {e}").to_response(status_code=401)
    except ForbiddenError as e:
        return ActionResponse.failure(f"Insufficient permissions: {e}").to_response(status_code=403)
    except SalesforceClientError as e:
        return ActionResponse.failure(f"Salesforce API error: {e}").to_response(status_code=502)


async def _create_case(
    credentials: Mapping[str, object], raw_params: Mapping[str, object]
) -> JSONResponse:
    params = CreateCaseParams.from_mapping(raw_params)
    payload: dict[str, str] = {"Subject": params.subject}
    if params.description is not None:
        payload["Description"] = params.description
    if params.status is not None:
        payload["Status"] = params.status
    if params.priority is not None:
        payload["Priority"] = params.priority
    if params.type is not None:
        payload["Type"] = params.type
    if params.origin is not None:
        payload["Origin"] = params.origin
    if params.account_id is not None:
        payload["AccountId"] = params.account_id
    if params.contact_id is not None:
        payload["ContactId"] = params.contact_id

    client = _client_from_credentials(credentials)
    try:
        case_id = await client.create("Case", payload)
        return ActionResponse.success({"case_id": case_id}).to_response(status_code=201)
    except AuthenticationError as e:
        return ActionResponse.failure(f"Authentication failed: {e}").to_response(status_code=401)
    except ForbiddenError as e:
        return ActionResponse.failure(f"Insufficient permissions: {e}").to_response(status_code=403)
    except SalesforceClientError as e:
        return ActionResponse.failure(f"Salesforce API error: {e}").to_response(status_code=502)


async def _update_case(
    credentials: Mapping[str, object], raw_params: Mapping[str, object]
) -> JSONResponse:
    params = UpdateCaseParams.from_mapping(raw_params)
    payload: dict[str, str] = {"Status": params.status}
    if params.priority is not None:
        payload["Priority"] = params.priority

    client = _client_from_credentials(credentials)
    try:
        await client.update("Case", params.case_id, payload)
        return ActionResponse.success(
            {"case_id": params.case_id, "status": params.status}
        ).to_response()
    except NotFoundError:
        return ActionResponse.failure(f"Case not found: {params.case_id}").to_response(
            status_code=404
        )
    except AuthenticationError as e:
        return ActionResponse.failure(f"Authentication failed: {e}").to_response(status_code=401)
    except ForbiddenError as e:
        return ActionResponse.failure(f"Insufficient permissions: {e}").to_response(status_code=403)
    except SalesforceClientError as e:
        return ActionResponse.failure(f"Salesforce API error: {e}").to_response(status_code=502)


async def _create_task(
    credentials: Mapping[str, object], raw_params: Mapping[str, object]
) -> JSONResponse:
    params = CreateTaskParams.from_mapping(raw_params)
    payload: dict[str, str] = {"Subject": params.subject}
    if params.description is not None:
        payload["Description"] = params.description
    if params.status is not None:
        payload["Status"] = params.status
    if params.priority is not None:
        payload["Priority"] = params.priority
    if params.activity_date is not None:
        payload["ActivityDate"] = params.activity_date
    if params.who_id is not None:
        payload["WhoId"] = params.who_id
    if params.what_id is not None:
        payload["WhatId"] = params.what_id

    client = _client_from_credentials(credentials)
    try:
        task_id = await client.create("Task", payload)
        return ActionResponse.success({"task_id": task_id}).to_response(status_code=201)
    except AuthenticationError as e:
        return ActionResponse.failure(f"Authentication failed: {e}").to_response(status_code=401)
    except ForbiddenError as e:
        return ActionResponse.failure(f"Insufficient permissions: {e}").to_response(status_code=403)
    except SalesforceClientError as e:
        return ActionResponse.failure(f"Salesforce API error: {e}").to_response(status_code=502)


async def _update_task(
    credentials: Mapping[str, object], raw_params: Mapping[str, object]
) -> JSONResponse:
    params = UpdateTaskParams.from_mapping(raw_params)
    client = _client_from_credentials(credentials)
    try:
        await client.update("Task", params.task_id, {"Status": params.status})
        return ActionResponse.success(
            {"task_id": params.task_id, "status": params.status}
        ).to_response()
    except NotFoundError:
        return ActionResponse.failure(f"Task not found: {params.task_id}").to_response(
            status_code=404
        )
    except AuthenticationError as e:
        return ActionResponse.failure(f"Authentication failed: {e}").to_response(status_code=401)
    except ForbiddenError as e:
        return ActionResponse.failure(f"Insufficient permissions: {e}").to_response(status_code=403)
    except SalesforceClientError as e:
        return ActionResponse.failure(f"Salesforce API error: {e}").to_response(status_code=502)


async def execute_action(
    action: str,
    params: Mapping[str, object],
    credentials: Mapping[str, object],
) -> JSONResponse:
    """Dispatch an action by name with typed params."""
    try:
        if action == "find_records":
            return await _find_records(credentials, params)
        if action == "get_case":
            return await _get_case(credentials, params)
        if action == "create_case":
            return await _create_case(credentials, params)
        if action == "update_case_status":
            return await _update_case(credentials, params)
        if action == "create_task":
            return await _create_task(credentials, params)
        if action == "update_task_status":
            return await _update_task(credentials, params)
    except ValueError as e:
        return ActionResponse.failure(str(e)).to_response(status_code=400)
    return ActionResponse.not_supported(action).to_response(status_code=404)
