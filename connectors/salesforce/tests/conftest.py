"""Integration test fixtures for the Salesforce connector.

Session-scoped: harness, mock Salesforce API server, connector server, connector-manager.
Function-scoped: seed helper, source_id, httpx client.
"""

from __future__ import annotations

import logging
import re
import socket
import threading
import time
from collections.abc import Mapping
from dataclasses import dataclass
from datetime import UTC, datetime
from typing import Any

import httpx
import pytest
import pytest_asyncio
import uvicorn
from omni_connector.testing import OmniTestHarness, SeedHelper
from starlette.applications import Starlette
from starlette.requests import Request
from starlette.responses import JSONResponse
from starlette.routing import Route

logger = logging.getLogger(__name__)


def _now_modstamp() -> str:
    return datetime.now(UTC).strftime("%Y-%m-%dT%H:%M:%S.000+0000")


def _parse_ts(value: str) -> datetime:
    return datetime.fromisoformat(value.replace("Z", "+00:00"))


# ---------------------------------------------------------------------------
# Record payload builders
# ---------------------------------------------------------------------------


def _record_payload(
    object_type: str,
    record_id: str,
    fields: dict[str, object],
) -> dict[str, object]:
    return {
        "attributes": {
            "type": object_type,
            "url": f"/services/data/v62.0/sobjects/{object_type}/{record_id}",
        },
        "Id": record_id,
        **fields,
    }


def _account_payload(
    record_id: str = "001000000000001",
    name: str = "Acme Corp",
    system_modstamp: str = "2024-06-01T14:00:00.000+0000",
    owner_id: str = "005000000000001",
) -> dict[str, object]:
    return _record_payload(
        "Account",
        record_id,
        {
            "Name": name,
            "Industry": "Technology",
            "Phone": "+1234567890",
            "Website": "https://acme.com",
            "BillingCity": "San Francisco",
            "BillingState": "CA",
            "BillingCountry": "US",
            "NumberOfEmployees": 50,
            "AnnualRevenue": 1000000,
            "Description": "A technology company",
            "Type": "Customer",
            "OwnerId": owner_id,
            "CreatedDate": "2024-01-15T10:30:00.000+0000",
            "SystemModstamp": system_modstamp,
        },
    )


def _contact_payload(
    record_id: str = "003000000000001",
    first_name: str = "John",
    last_name: str = "Doe",
    email: str = "john@example.com",
    system_modstamp: str = "2024-06-01T14:00:00.000+0000",
    owner_id: str = "005000000000001",
    account_name: str = "Acme Corp",
) -> dict[str, object]:
    return _record_payload(
        "Contact",
        record_id,
        {
            "FirstName": first_name,
            "LastName": last_name,
            "Name": f"{first_name} {last_name}",
            "Email": email,
            "Phone": "+1234567890",
            "Title": "Engineer",
            "Department": "Engineering",
            "AccountId": "001000000000001",
            "Account": {"Name": account_name} if account_name else None,
            "MailingCity": "San Francisco",
            "MailingState": "CA",
            "MailingCountry": "US",
            "OwnerId": owner_id,
            "CreatedDate": "2024-01-15T10:30:00.000+0000",
            "SystemModstamp": system_modstamp,
        },
    )


def _opportunity_payload(
    record_id: str = "006000000000001",
    name: str = "Big Deal",
    system_modstamp: str = "2024-06-10T11:00:00.000+0000",
    owner_id: str = "005000000000001",
) -> dict[str, object]:
    return _record_payload(
        "Opportunity",
        record_id,
        {
            "Name": name,
            "Amount": 50000,
            "StageName": "Prospecting",
            "CloseDate": "2024-12-31",
            "Probability": 25,
            "Type": "New Business",
            "LeadSource": "Web",
            "Description": "A big deal",
            "AccountId": "001000000000001",
            "Account": {"Name": "Acme Corp"},
            "OwnerId": owner_id,
            "CreatedDate": "2024-02-01T09:00:00.000+0000",
            "SystemModstamp": system_modstamp,
        },
    )


def _lead_payload(
    record_id: str = "00Q000000000001",
    first_name: str = "Jane",
    last_name: str = "Smith",
    system_modstamp: str = "2024-03-15T16:00:00.000+0000",
    owner_id: str = "005000000000001",
) -> dict[str, object]:
    return _record_payload(
        "Lead",
        record_id,
        {
            "FirstName": first_name,
            "LastName": last_name,
            "Name": f"{first_name} {last_name}",
            "Email": "jane@example.com",
            "Phone": "+1987654321",
            "Company": "StartupCo",
            "Title": "CTO",
            "Industry": "Software",
            "Status": "Open",
            "LeadSource": "Web",
            "Description": "Interested in our product",
            "OwnerId": owner_id,
            "CreatedDate": "2024-03-01T10:00:00.000+0000",
            "SystemModstamp": system_modstamp,
        },
    )


def _case_payload(
    record_id: str = "500000000000001",
    subject: str = "Support request",
    system_modstamp: str = "2024-04-01T15:00:00.000+0000",
    owner_id: str = "005000000000001",
) -> dict[str, object]:
    return _record_payload(
        "Case",
        record_id,
        {
            "Subject": subject,
            "Description": "Need help with integration",
            "Status": "New",
            "Priority": "High",
            "Type": "Problem",
            "Origin": "Web",
            "ContactId": "003000000000001",
            "AccountId": "001000000000001",
            "Account": {"Name": "Acme Corp"},
            "OwnerId": owner_id,
            "CreatedDate": "2024-04-01T14:00:00.000+0000",
            "SystemModstamp": system_modstamp,
        },
    )


def _task_payload(
    record_id: str = "00T000000000001",
    subject: str = "Send proposal",
    system_modstamp: str = "2024-04-20T09:00:00.000+0000",
    owner_id: str = "005000000000001",
) -> dict[str, object]:
    return _record_payload(
        "Task",
        record_id,
        {
            "Subject": subject,
            "Description": "Prepare and send the proposal",
            "Status": "Not Started",
            "Priority": "High",
            "ActivityDate": "2024-04-20",
            "WhoId": "003000000000001",
            "WhatId": "006000000000001",
            "OwnerId": owner_id,
            "CreatedDate": "2024-04-20T09:00:00.000+0000",
            "SystemModstamp": system_modstamp,
        },
    )


def _user_payload(
    user_id: str = "005000000000001",
    email: str = "owner@example.com",
    name: str = "Owner User",
    is_active: bool = True,
    role_id: str | None = "00E000000000001",
    manager_id: str | None = None,
) -> dict[str, object]:
    return _record_payload(
        "User",
        user_id,
        {
            "Name": name,
            "FirstName": name.split()[0] if name else None,
            "LastName": " ".join(name.split()[1:]) if name else None,
            "Email": email,
            "Title": "Sales Rep",
            "Department": "Sales",
            "ManagerId": manager_id,
            "UserRoleId": role_id,
            "IsActive": is_active,
            "EmployeeNumber": user_id[-4:],
            "SystemModstamp": "2024-05-01T09:00:00.000+0000",
        },
    )


def _group_payload(
    group_id: str = "00G000000000001",
    name: str = "Support Queue",
    group_type: str = "Queue",
) -> dict[str, object]:
    return _record_payload(
        "Group",
        group_id,
        {"Name": name, "Type": group_type},
    )


def _group_member_payload(
    member_id: str,
    group_id: str,
    user_or_group_id: str,
) -> dict[str, object]:
    return _record_payload(
        "GroupMember",
        member_id,
        {"GroupId": group_id, "UserOrGroupId": user_or_group_id},
    )


def _role_payload(
    role_id: str = "00E000000000001",
    name: str = "Sales Manager",
    parent_role_id: str | None = None,
) -> dict[str, object]:
    return _record_payload(
        "UserRole",
        role_id,
        {"Name": name, "ParentRoleId": parent_role_id},
    )


def _share_payload(
    object_type: str,
    share_id: str,
    parent_field: str,
    parent_id: str,
    user_or_group_id: str,
    access_level: str = "Read",
    row_cause: str = "Manual",
) -> dict[str, object]:
    return _record_payload(
        object_type,
        share_id,
        {
            parent_field: parent_id,
            "UserOrGroupId": user_or_group_id,
            "AccessLevel": access_level,
            "RowCause": row_cause,
        },
    )


# ---------------------------------------------------------------------------
# Minimal SOQL handling for the mock
# ---------------------------------------------------------------------------


@dataclass(frozen=True)
class ParsedSoql:
    object_type: str
    where: str | None
    order_by: tuple[str, ...]
    limit: int | None


def _parse_soql(soql: str) -> ParsedSoql:
    object_match = re.search(r"FROM\s+(\w+)", soql, re.IGNORECASE)
    if not object_match:
        raise ValueError(f"Invalid SOQL: {soql}")
    object_type = object_match.group(1)
    where_match = re.search(r"\bWHERE\s+(.+?)(?:\s+ORDER BY|\s+LIMIT|$)", soql, re.IGNORECASE)
    where = where_match.group(1).strip() if where_match else None
    order_match = re.search(r"ORDER BY\s+(.+?)(?:\s+LIMIT|$)", soql, re.IGNORECASE)
    order_by = (
        tuple(
            part.strip().removesuffix(" ASC").removesuffix(" DESC")
            for part in order_match.group(1).split(",")
        )
        if order_match
        else ()
    )
    limit_match = re.search(r"LIMIT\s+(\d+)", soql, re.IGNORECASE)
    limit = int(limit_match.group(1)) if limit_match else None
    return ParsedSoql(object_type, where, order_by, limit)


def _soql_value(record: Mapping[str, object], field: str) -> object:
    return record.get(field)


def _matches_where(record: Mapping[str, object], where: str) -> bool:
    # Keyset delta clause: (SystemModstamp > X OR (SystemModstamp = X AND Id > 'Y'))
    keyset = re.search(
        r"SystemModstamp\s*>\s*([^ )]+)\s*OR\s*\(\s*SystemModstamp\s*=\s*([^ )]+)"
        r"\s*AND\s+Id\s*>\s*'([^']+)'\s*\)",
        where,
    )
    if keyset:
        threshold = _parse_ts(keyset.group(1))
        modstamp = _soql_value(record, "SystemModstamp")
        if modstamp is None:
            return False
        value = _parse_ts(str(modstamp))
        if value > threshold:
            return True
        if value == threshold and str(record.get("Id", "")) > keyset.group(3):
            return True
        return False

    id_gt = re.search(r"Id\s*>\s*'([^']+)'", where)
    if id_gt and not re.search(r"SystemModstamp", where):
        return str(record.get("Id", "")) > id_gt.group(1)

    id_in = re.search(r"Id\s+IN\s*\(([^)]+)\)", where)
    if id_in:
        ids = {v.strip().strip("'") for v in id_in.group(1).split(",")}
        return record.get("Id") in ids

    type_in = re.search(r"Type\s+IN\s*\(([^)]+)\)", where)
    if type_in:
        types = {v.strip().strip("'") for v in type_in.group(1).split(",")}
        return record.get("Type") in types

    row_cause = re.search(r"RowCause\s*!=\s*'([^']+)'", where)
    if row_cause:
        return record.get("RowCause") != row_cause.group(1)

    mod_ge = re.search(r"SystemModstamp\s*>=\s*([^ )]+)", where)
    if mod_ge:
        threshold = _parse_ts(mod_ge.group(1))
        modstamp = _soql_value(record, "SystemModstamp")
        if modstamp is None:
            return False
        return _parse_ts(str(modstamp)) >= threshold

    # LIKE branches: "Name LIKE '%x%' OR Email LIKE '%y%'"
    like_branches = re.split(r"\s+OR\s+", where)
    saw_like = False
    for branch in like_branches:
        like = re.search(r"(\w+)\s+LIKE\s+'%([^']*)%'", branch)
        if like:
            saw_like = True
            value = _soql_value(record, like.group(1))
            if isinstance(value, str) and like.group(2).lower() in value.lower():
                return True
    if saw_like:
        return False
    return True


def _matches_ts_window(
    record: Mapping[str, object], field: str, start: datetime, end: datetime
) -> bool:
    value = record.get(field)
    if value is None:
        return False
    try:
        parsed = _parse_ts(str(value))
    except ValueError:
        return False
    return start <= parsed <= end


# ---------------------------------------------------------------------------
# Mock Salesforce API
# ---------------------------------------------------------------------------


class MockSalesforceAPI:
    """Controllable mock of the Salesforce REST API with SOQL filtering."""

    def __init__(self) -> None:
        self.objects: dict[str, list[dict[str, object]]] = {}
        self.deleted: dict[str, list[dict[str, str]]] = {}
        self.should_fail_auth: bool = False
        self.created_records: list[dict[str, object]] = []
        self.updated_records: list[tuple[str, str]] = []
        self.next_record_id = 1

    def reset(self) -> None:
        self.objects.clear()
        self.deleted.clear()
        self.should_fail_auth = False
        self.created_records.clear()
        self.updated_records.clear()
        self.next_record_id = 1

    def add_record(self, object_type: str, payload: dict[str, object]) -> None:
        self.objects.setdefault(object_type, []).append(payload)

    def add_account(self, record_id: str = "001000000000001", **kwargs: Any) -> None:
        self.add_record("Account", _account_payload(record_id, **kwargs))

    def add_contact(self, record_id: str = "003000000000001", **kwargs: Any) -> None:
        self.add_record("Contact", _contact_payload(record_id, **kwargs))

    def add_opportunity(self, record_id: str = "006000000000001", **kwargs: Any) -> None:
        self.add_record("Opportunity", _opportunity_payload(record_id, **kwargs))

    def add_lead(self, record_id: str = "00Q000000000001", **kwargs: Any) -> None:
        self.add_record("Lead", _lead_payload(record_id, **kwargs))

    def add_case(self, record_id: str = "500000000000001", **kwargs: Any) -> None:
        self.add_record("Case", _case_payload(record_id, **kwargs))

    def add_task(self, record_id: str = "00T000000000001", **kwargs: Any) -> None:
        self.add_record("Task", _task_payload(record_id, **kwargs))

    def add_user(self, user_id: str = "005000000000001", **kwargs: Any) -> None:
        self.add_record("User", _user_payload(user_id, **kwargs))

    def add_group(self, group_id: str = "00G000000000001", **kwargs: Any) -> None:
        self.add_record("Group", _group_payload(group_id, **kwargs))

    def add_group_member(self, member_id: str, group_id: str, user_or_group_id: str) -> None:
        self.add_record("GroupMember", _group_member_payload(member_id, group_id, user_or_group_id))

    def add_role(self, role_id: str = "00E000000000001", **kwargs: Any) -> None:
        self.add_record("UserRole", _role_payload(role_id, **kwargs))

    def add_share(self, object_type: str, **kwargs: Any) -> None:
        parent_field = {
            "AccountShare": "AccountId",
            "ContactShare": "ContactId",
            "OpportunityShare": "OpportunityId",
            "LeadShare": "LeadId",
            "CaseShare": "CaseId",
        }[object_type]
        self.add_record(
            object_type,
            _share_payload(
                object_type, f"{object_type}-{self.next_record_id}", parent_field, **kwargs
            ),
        )
        self.next_record_id += 1

    def add_people_fixtures(self) -> None:
        """Default org: 2 users, a queue, a public group, and a role hierarchy."""
        self.add_user("005000000000001", email="owner@example.com", name="Owner User")
        self.add_user(
            "005000000000002",
            email="agent@example.com",
            name="Support Agent",
            role_id="00E000000000002",
        )
        self.add_user(
            "005000000000003",
            email="manager@example.com",
            name="Sales Manager",
            role_id="00E000000000003",
        )
        self.add_role("00E000000000002", name="Support Rep", parent_role_id="00E000000000003")
        self.add_role("00E000000000003", name="Support Manager")
        self.add_group("00G000000000001", name="Support Queue", group_type="Queue")
        self.add_group_member("00M000000000001", "00G000000000001", "005000000000002")
        self.add_group("00G000000000002", name="Execs", group_type="Public")
        self.add_group_member("00M000000000002", "00G000000000002", "005000000000003")

    def mark_deleted(self, object_type: str, record_id: str) -> None:
        self.deleted.setdefault(object_type, []).append(
            {"id": record_id, "deletedDate": _now_modstamp()}
        )

    def _query_records(self, soql: str) -> list[dict[str, object]]:
        parsed = _parse_soql(soql)
        records = [
            record
            for record in self.objects.get(parsed.object_type, [])
            if parsed.where is None or _matches_where(record, parsed.where)
        ]
        if parsed.order_by:

            def sort_key(record: Mapping[str, object]) -> tuple[object, ...]:
                return tuple(record.get(field) for field in parsed.order_by)

            records.sort(key=sort_key)
        if parsed.limit is not None:
            records = records[: parsed.limit]
        return records

    def create_app(self) -> Starlette:
        mock = self

        def auth_guard() -> JSONResponse | None:
            if mock.should_fail_auth:
                return JSONResponse(
                    [
                        {
                            "message": "Session expired or invalid",
                            "errorCode": "INVALID_SESSION_ID",
                        }
                    ],
                    status_code=401,
                )
            return None

        async def handle_query(request: Request) -> JSONResponse:
            denied = auth_guard()
            if denied:
                return denied
            soql = request.query_params.get("q", "")
            try:
                records = mock._query_records(soql)
            except ValueError as e:
                return JSONResponse(
                    [{"message": str(e), "errorCode": "MALFORMED_QUERY"}],
                    status_code=400,
                )
            return JSONResponse({"totalSize": len(records), "done": True, "records": records})

        async def handle_updated(request: Request) -> JSONResponse:
            denied = auth_guard()
            if denied:
                return denied
            object_type = request.path_params["object_type"]
            start = _parse_ts(request.query_params["start"])
            end = _parse_ts(request.query_params["end"])
            ids = [
                str(record["Id"])
                for record in mock.objects.get(object_type, [])
                if _matches_ts_window(record, "SystemModstamp", start, end)
            ]
            return JSONResponse(
                {
                    "ids": ids,
                    "latestDateCovered": end.strftime("%Y-%m-%dT%H:%M:%SZ"),
                }
            )

        async def handle_deleted(request: Request) -> JSONResponse:
            denied = auth_guard()
            if denied:
                return denied
            object_type = request.path_params["object_type"]
            start = _parse_ts(request.query_params["start"])
            end = _parse_ts(request.query_params["end"])
            deleted_records = [
                entry
                for entry in mock.deleted.get(object_type, [])
                if start <= _parse_ts(entry["deletedDate"]) <= end
            ]
            return JSONResponse(
                {
                    "deletedRecords": deleted_records,
                    "earliestDateAvailable": start.strftime("%Y-%m-%dT%H:%M:%SZ"),
                    "latestDateCovered": end.strftime("%Y-%m-%dT%H:%M:%SZ"),
                }
            )

        async def handle_get_record(request: Request) -> JSONResponse:
            denied = auth_guard()
            if denied:
                return denied
            object_type = request.path_params["object_type"]
            record_id = request.path_params["record_id"]
            for record in mock.objects.get(object_type, []):
                if record.get("Id") == record_id:
                    return JSONResponse(record)
            return JSONResponse(
                [{"message": f"{object_type} not found", "errorCode": "NOT_FOUND"}],
                status_code=404,
            )

        async def handle_create_record(request: Request) -> JSONResponse:
            denied = auth_guard()
            if denied:
                return denied
            object_type = request.path_params["object_type"]
            body = await request.json()
            if not isinstance(body, Mapping):
                return JSONResponse({"message": "invalid body"}, status_code=400)
            record_id = str(body.get("Id") or f"00R{mock.next_record_id:013d}")
            mock.next_record_id += 1
            record = _record_payload(
                object_type,
                record_id,
                {**dict(body), "SystemModstamp": _now_modstamp()},
            )
            mock.add_record(object_type, record)
            mock.created_records.append(record)
            return JSONResponse({"id": record_id, "success": True}, status_code=201)

        async def handle_update_record(request: Request) -> JSONResponse:
            denied = auth_guard()
            if denied:
                return denied
            object_type = request.path_params["object_type"]
            record_id = request.path_params["record_id"]
            body = await request.json()
            if not isinstance(body, Mapping):
                return JSONResponse({"message": "invalid body"}, status_code=400)
            for record in mock.objects.get(object_type, []):
                if record.get("Id") == record_id:
                    record.update(body)
                    record["SystemModstamp"] = _now_modstamp()
                    mock.updated_records.append((object_type, record_id))
                    return JSONResponse({}, status_code=204)
            return JSONResponse(
                [{"message": f"{object_type} not found", "errorCode": "NOT_FOUND"}],
                status_code=404,
            )

        routes = [
            Route("/services/data/v62.0/query/", handle_query),
            Route(
                "/services/data/v62.0/sobjects/{object_type}/updated",
                handle_updated,
            ),
            Route(
                "/services/data/v62.0/sobjects/{object_type}/deleted",
                handle_deleted,
            ),
            Route(
                "/services/data/v62.0/sobjects/{object_type}/",
                handle_create_record,
                methods=["POST"],
            ),
            Route(
                "/services/data/v62.0/sobjects/{object_type}/{record_id}",
                handle_update_record,
                methods=["PATCH"],
            ),
            Route(
                "/services/data/v62.0/sobjects/{object_type}/{record_id}",
                handle_get_record,
                methods=["GET"],
            ),
        ]
        return Starlette(routes=routes)


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _free_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
        s.bind(("", 0))
        return s.getsockname()[1]


def _wait_for_port(port: int, host: str = "localhost", timeout: float = 10) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            with socket.create_connection((host, port), timeout=1):
                return
        except OSError:
            time.sleep(0.1)
    raise TimeoutError(f"Port {port} not open after {timeout}s")


# ---------------------------------------------------------------------------
# Session-scoped fixtures
# ---------------------------------------------------------------------------


@pytest.fixture(scope="session")
def mock_salesforce_api() -> MockSalesforceAPI:
    return MockSalesforceAPI()


@pytest.fixture(scope="session")
def mock_salesforce_server(mock_salesforce_api: MockSalesforceAPI) -> str:
    """Start mock Salesforce API server in a daemon thread. Returns base URL."""
    port = _free_port()
    app = mock_salesforce_api.create_app()
    config = uvicorn.Config(app, host="0.0.0.0", port=port, log_level="warning")
    server = uvicorn.Server(config)

    thread = threading.Thread(target=server.run, daemon=True)
    thread.start()

    _wait_for_port(port)
    return f"http://localhost:{port}"


@pytest.fixture(scope="session")
def connector_port() -> int:
    return _free_port()


@pytest.fixture(scope="session")
def connector_server(connector_port: int) -> str:
    """Start the Salesforce connector as a uvicorn server in a daemon thread. Returns base URL."""
    import os

    os.environ.setdefault("CONNECTOR_MANAGER_URL", "http://localhost:0")
    os.environ.setdefault("CONNECTOR_HOST_NAME", "host.docker.internal")
    os.environ.setdefault("PORT", str(connector_port))

    from omni_connector.server import create_app

    from salesforce_connector import SalesforceConnector

    app = create_app(SalesforceConnector())
    config = uvicorn.Config(app, host="0.0.0.0", port=connector_port, log_level="warning")
    server = uvicorn.Server(config)

    thread = threading.Thread(target=server.run, daemon=True)
    thread.start()

    _wait_for_port(connector_port)
    return f"http://localhost:{connector_port}"


@pytest_asyncio.fixture(scope="session")
async def harness(
    connector_server: str,
    connector_port: int,
) -> OmniTestHarness:
    """Session-scoped OmniTestHarness with all infrastructure started."""
    import os

    h = OmniTestHarness()
    await h.start_infra()
    await h.start_connector_manager(
        {
            "SALESFORCE_CONNECTOR_URL": f"http://host.docker.internal:{connector_port}",
        }
    )

    os.environ["CONNECTOR_MANAGER_URL"] = h.connector_manager_url

    yield h
    await h.teardown()


# ---------------------------------------------------------------------------
# Function-scoped fixtures
# ---------------------------------------------------------------------------


@pytest_asyncio.fixture
async def seed(harness: OmniTestHarness) -> SeedHelper:
    return harness.seed()


@pytest_asyncio.fixture
async def source_id(
    seed: SeedHelper,
    mock_salesforce_server: str,
    mock_salesforce_api: MockSalesforceAPI,
) -> str:
    """Create a Salesforce source with credentials pointing to the mock server."""
    mock_salesforce_api.reset()
    sid = await seed.create_source(
        source_type="salesforce",
        config={"instance_url": mock_salesforce_server},
    )
    await seed.create_credentials(
        sid,
        {"access_token": "test-token", "instance_url": mock_salesforce_server},
        provider="salesforce",
    )
    return sid


@pytest_asyncio.fixture
async def cm_client(harness: OmniTestHarness) -> httpx.AsyncClient:
    """Async httpx client pointed at the connector-manager."""
    async with httpx.AsyncClient(base_url=harness.connector_manager_url, timeout=30) as client:
        yield client
