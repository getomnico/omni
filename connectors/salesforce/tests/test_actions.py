"""Integration tests: connector actions via the connector HTTP API."""

from __future__ import annotations

import httpx
import pytest
import pytest_asyncio

from tests.conftest import _now_modstamp

pytestmark = pytest.mark.integration


@pytest_asyncio.fixture
async def action_client(connector_server: str) -> httpx.AsyncClient:
    async with httpx.AsyncClient(base_url=connector_server, timeout=30) as client:
        yield client


def _credentials(mock_salesforce_server: str) -> dict[str, str]:
    return {"access_token": "test-token", "instance_url": mock_salesforce_server}


async def test_manifest_declares_actions_and_operators(
    action_client: httpx.AsyncClient,
) -> None:
    resp = await action_client.get("/manifest")
    assert resp.status_code == 200
    manifest = resp.json()
    assert manifest["source_types"] == ["salesforce"]
    assert set(manifest["sync_modes"]) == {"full", "incremental", "realtime"}
    action_names = {a["name"] for a in manifest["actions"]}
    assert {
        "find_records",
        "get_case",
        "create_case",
        "update_case_status",
        "create_task",
        "update_task_status",
    } <= action_names
    operator_map = {op["operator"]: op["attribute_key"] for op in manifest["search_operators"]}
    assert operator_map["owner"] == "owner_email"
    assert operator_map["status"] == "status"
    assert operator_map["priority"] == "priority"
    assert operator_map["stage"] == "stage"


async def test_find_records(
    mock_salesforce_api, mock_salesforce_server, action_client: httpx.AsyncClient
) -> None:
    mock_salesforce_api.reset()
    mock_salesforce_api.add_account()
    mock_salesforce_api.add_account("001000000000002", name="Beta Corp")

    resp = await action_client.post(
        "/action",
        json={
            "action": "find_records",
            "params": {"object_type": "Account", "query": "acme"},
            "credentials": _credentials(mock_salesforce_server),
        },
    )
    assert resp.status_code == 200, resp.text
    data = resp.json()
    assert data["status"] == "success"
    assert data["result"]["object_type"] == "Account"
    records = data["result"]["records"]
    assert [r["name"] for r in records] == ["Acme Corp"]


async def test_create_case(
    mock_salesforce_api, mock_salesforce_server, action_client: httpx.AsyncClient
) -> None:
    mock_salesforce_api.reset()
    resp = await action_client.post(
        "/action",
        json={
            "action": "create_case",
            "params": {"subject": "New issue from agent", "priority": "High"},
            "credentials": _credentials(mock_salesforce_server),
        },
    )
    assert resp.status_code == 201, resp.text
    case_id = resp.json()["result"]["case_id"]
    created = [r for r in mock_salesforce_api.objects.get("Case", []) if r.get("Id") == case_id]
    assert len(created) == 1
    assert created[0]["Subject"] == "New issue from agent"
    assert created[0]["Priority"] == "High"


async def test_update_case_status(
    mock_salesforce_api, mock_salesforce_server, action_client: httpx.AsyncClient
) -> None:
    mock_salesforce_api.reset()
    mock_salesforce_api.add_case()

    resp = await action_client.post(
        "/action",
        json={
            "action": "update_case_status",
            "params": {"case_id": "500000000000001", "status": "Closed"},
            "credentials": _credentials(mock_salesforce_server),
        },
    )
    assert resp.status_code == 200, resp.text
    assert resp.json()["status"] == "success"
    case = [
        r for r in mock_salesforce_api.objects.get("Case", []) if r.get("Id") == "500000000000001"
    ][0]
    assert case["Status"] == "Closed"


async def test_update_task_status(
    mock_salesforce_api, mock_salesforce_server, action_client: httpx.AsyncClient
) -> None:
    mock_salesforce_api.reset()
    mock_salesforce_api.add_task(system_modstamp=_now_modstamp())

    resp = await action_client.post(
        "/action",
        json={
            "action": "update_task_status",
            "params": {"task_id": "00T000000000001", "status": "Completed"},
            "credentials": _credentials(mock_salesforce_server),
        },
    )
    assert resp.status_code == 200, resp.text
    task = [
        r for r in mock_salesforce_api.objects.get("Task", []) if r.get("Id") == "00T000000000001"
    ][0]
    assert task["Status"] == "Completed"


async def test_missing_required_param_is_400(
    mock_salesforce_server, action_client: httpx.AsyncClient
) -> None:
    resp = await action_client.post(
        "/action",
        json={
            "action": "create_case",
            "params": {},
            "credentials": _credentials(mock_salesforce_server),
        },
    )
    assert resp.status_code == 400, resp.text


async def test_unknown_action_is_404(
    mock_salesforce_server, action_client: httpx.AsyncClient
) -> None:
    resp = await action_client.post(
        "/action",
        json={
            "action": "no_such_action",
            "params": {},
            "credentials": _credentials(mock_salesforce_server),
        },
    )
    assert resp.status_code == 404, resp.text
