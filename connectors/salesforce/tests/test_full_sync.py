"""Integration tests: full sync emits documents, people, and group memberships."""

from __future__ import annotations

import httpx
import pytest
from omni_connector.testing import count_events, get_events, wait_for_sync

from salesforce_connector.models import group_email, role_email

pytestmark = pytest.mark.integration


async def _run_sync(cm_client: httpx.AsyncClient, source_id: str, sync_type: str) -> dict:
    resp = await cm_client.post("/sync", json={"source_id": source_id, "sync_type": sync_type})
    assert resp.status_code == 200, resp.text
    return resp.json()


async def _sync_docs(harness, cm_client, source_id, sync_type="full") -> dict:
    run = await _run_sync(cm_client, source_id, sync_type)
    row = await wait_for_sync(harness.db_pool, run["sync_run_id"], timeout=40)
    assert row["status"] == "completed", f"status={row['status']} err={row.get('error_message')}"
    events = await get_events(harness.db_pool, source_id)
    docs: dict[str, dict] = {}
    for event in events:
        payload = event["payload"]
        if payload.get("type") == "document_created":
            docs[payload["document_id"]] = payload
    return docs


async def test_full_sync_emits_documents_with_permissions(
    harness, seed, source_id, mock_salesforce_api, cm_client: httpx.AsyncClient
) -> None:
    mock_salesforce_api.add_people_fixtures()
    mock_salesforce_api.add_account()
    mock_salesforce_api.add_contact()
    mock_salesforce_api.add_opportunity()
    mock_salesforce_api.add_lead()
    mock_salesforce_api.add_case()
    mock_salesforce_api.add_task()
    # Share the account with manager@example.com; the case with the Execs group.
    mock_salesforce_api.add_share(
        "AccountShare", parent_id="001000000000001", user_or_group_id="005000000000003"
    )
    mock_salesforce_api.add_share(
        "CaseShare", parent_id="500000000000001", user_or_group_id="00G000000000002"
    )

    docs = await _sync_docs(harness, cm_client, source_id)

    assert set(docs) >= {
        "Account:001000000000001",
        "Contact:003000000000001",
        "Opportunity:006000000000001",
        "Lead:00Q000000000001",
        "Case:500000000000001",
        "Task:00T000000000001",
    }

    # Account: org-wide public read by default, owned by owner@example.com,
    # and shared with manager@example.com.
    account = docs["Account:001000000000001"]
    assert account["permissions"]["public"] is True
    assert "owner@example.com" in account["permissions"]["users"]
    assert (
        "manager@example.com" in account["permissions"]["users"]
        or "manager@example.com" in account["permissions"]["groups"]
    )
    assert account["attributes"]["industry"] == "Technology"
    assert account["attributes"]["account_name"] == "Acme Corp"
    assert account["attributes"]["owner_email"] == "owner@example.com"
    assert account["attributes"]["object_type"] == "Account"

    # Case: priority/status attributes and role-hierarchy groups for the owner.
    case = docs["Case:500000000000001"]
    assert case["attributes"]["status"] == "New"
    assert case["attributes"]["priority"] == "High"
    assert case["permissions"]["public"] is False
    assert "owner@example.com" in case["permissions"]["users"]
    assert role_email("00E000000000002") in case["permissions"]["groups"]
    assert role_email("00E000000000003") in case["permissions"]["groups"]
    # Manual share to the Execs public group.
    assert group_email("00G000000000002") in case["permissions"]["groups"]


async def test_full_sync_emits_people_and_groups(
    harness, seed, source_id, mock_salesforce_api, cm_client: httpx.AsyncClient
) -> None:
    mock_salesforce_api.add_people_fixtures()
    mock_salesforce_api.add_account()

    run = await _run_sync(cm_client, source_id, "full")
    row = await wait_for_sync(harness.db_pool, run["sync_run_id"], timeout=40)
    assert row["status"] == "completed"

    events = await get_events(harness.db_pool, source_id)
    person_events = [e["payload"] for e in events if e["payload"]["type"] == "person_sync"]
    active_emails = {e["person"]["email"] for e in person_events}
    assert active_emails == {"owner@example.com", "agent@example.com", "manager@example.com"}

    group_payloads = [
        e["payload"] for e in events if e["payload"]["type"] == "group_membership_sync"
    ]
    memberships = {g["group_email"]: set(g["member_emails"]) for g in group_payloads}
    # Support Queue contains the agent.
    assert memberships.get(group_email("00G000000000001")) == {"agent@example.com"}
    # Execs public group contains the manager (and the manager role has no members).
    assert memberships.get(group_email("00G000000000002")) == {"manager@example.com"}
    # Support Rep role (00E...002) contains the owner; Support Manager role
    # (00E...003) contains the agent plus everyone below it (the owner).
    assert memberships.get(role_email("00E000000000002")) == {"owner@example.com"}
    assert memberships.get(role_email("00E000000000003")) == {
        "agent@example.com",
        "owner@example.com",
    }
    # Sales Manager role (00E...001) is the root: manager + everything below.
    assert memberships.get(role_email("00E000000000001")) == {
        "owner@example.com",
        "agent@example.com",
        "manager@example.com",
    }

    n_people = await count_events(harness.db_pool, source_id, "person_sync")
    assert n_people == 3


async def test_queue_owned_record_grants_queue_group(
    harness, seed, source_id, mock_salesforce_api, cm_client: httpx.AsyncClient
) -> None:
    mock_salesforce_api.add_people_fixtures()
    mock_salesforce_api.add_case(owner_id="00G000000000001")

    docs = await _sync_docs(harness, cm_client, source_id)

    case = docs["Case:500000000000001"]
    perms = case["permissions"]
    assert perms["users"] == []
    assert perms["groups"] == [group_email("00G000000000001")]
    assert perms["public"] is False


async def test_full_sync_reports_scanned_count(
    harness, seed, source_id, mock_salesforce_api, cm_client: httpx.AsyncClient
) -> None:
    mock_salesforce_api.add_people_fixtures()
    for i in range(3):
        mock_salesforce_api.add_account(f"00100000000000{i}")

    run = await _run_sync(cm_client, source_id, "full")
    row = await wait_for_sync(harness.db_pool, run["sync_run_id"], timeout=40)
    assert row["status"] == "completed"
    assert row["documents_scanned"] == 3
