"""Generic Salesforce Case action integration coverage."""

# ruff: noqa: E501

from __future__ import annotations

import json

import pytest

import salesforce_connector.case_actions as case_actions
from salesforce_connector.actions import execute_action
from salesforce_connector.case_actions import (
    create_case,
    fetch_case_file,
    get_case_detail,
    get_case_email_thread,
    get_case_options,
    get_case_requester_profile,
    list_case_files,
    list_case_queues,
    list_inbound_replies,
    list_new_cases,
    list_stale_cases,
    reply_to_case,
    route_case,
    update_case,
)
from salesforce_connector.client import (
    ObjectDescribe,
    SalesforceClientError,
    StandardActionResult,
)
from salesforce_connector.connector import SalesforceConnector
from salesforce_connector.models import SalesforceSourceConfig
from tests.conftest import _account_payload, _case_payload, _contact_payload, _user_payload

pytestmark = pytest.mark.integration


def _credentials(server: str) -> dict[str, str]:
    return {"access_token": "test-token", "instance_url": server}


def _config() -> SalesforceSourceConfig:
    return SalesforceSourceConfig()


async def test_case_options_create_update_and_queue_assignment(
    mock_salesforce_api, mock_salesforce_server: str
) -> None:
    mock_salesforce_api.reset()
    for index in range(1, 4):
        mock_salesforce_api.add_record(
            "RecordType",
            {
                "Id": f"01200000000000{index}",
                "Name": f"Support {index}",
                "SobjectType": "Case",
                "IsActive": True,
            },
        )
    mock_salesforce_api.add_group("00G000000000001", name="Support Queue", group_type="Queue")
    mock_salesforce_api.add_record(
        "QueueSobject",
        {"Id": "QUE000000000001", "QueueId": "00G000000000001", "SobjectType": "Case"},
    )
    options = await get_case_options(_credentials(mock_salesforce_server), {}, _config())
    assert options.status_code == 200
    directory = json.loads(options.body)["result"]["record_types"]
    assert "statuses" not in directory[0]
    selected = await get_case_options(
        _credentials(mock_salesforce_server),
        {"record_type_id": "012000000000001"},
        _config(),
    )
    assert json.loads(selected.body)["result"]["record_types"][0]["statuses"] == [
        "New",
        "Working",
        "Closed",
    ]
    assert len(mock_salesforce_api.picklist_requests) == 2
    invalid = await get_case_options(
        _credentials(mock_salesforce_server),
        {"record_type_id": "012000000000999"},
        _config(),
    )
    assert invalid.status_code == 400
    malformed = await get_case_options(
        _credentials(mock_salesforce_server),
        {"record_type_id": "001000000000001"},
        _config(),
    )
    assert malformed.status_code == 400

    created = await create_case(
        _credentials(mock_salesforce_server),
        {"subject": "Problem", "record_type_id": "012000000000001", "owner_id": "00G000000000001"},
        _config(),
    )
    assert created.status_code == 201
    case = _case_payload()
    mock_salesforce_api.add_record("Case", case)
    updated = await update_case(
        _credentials(mock_salesforce_server),
        {"case_id": case["Id"], "status": "Working", "description": None},
        _config(),
    )
    assert updated.status_code == 200
    routed = await route_case(
        _credentials(mock_salesforce_server),
        {"case_id": case["Id"], "queue_id": "00G000000000001"},
        _config(),
    )
    assert routed.status_code == 200


async def test_create_ambiguous_failure_is_not_replayed(
    mock_salesforce_api, mock_salesforce_server: str
) -> None:
    mock_salesforce_api.reset()
    mock_salesforce_api.create_5xx_remaining = 1
    response = await create_case(
        _credentials(mock_salesforce_server), {"subject": "Ambiguous"}, _config()
    )
    assert response.status_code == 502
    assert mock_salesforce_api.create_calls == 1
    assert not mock_salesforce_api.created_records


async def test_object_crud_and_queryability_boundaries_precede_side_effects(
    mock_salesforce_api, mock_salesforce_server: str
) -> None:
    mock_salesforce_api.reset()
    mock_salesforce_api.add_record("Case", _case_payload())
    mock_salesforce_api.object_capabilities["Case"] = {
        "queryable": False,
        "createable": True,
        "updateable": True,
    }
    unreadable = await get_case_detail(
        _credentials(mock_salesforce_server), {"case_id": "500000000000001"}, _config()
    )
    assert unreadable.status_code == 403
    assert not mock_salesforce_api.queries

    mock_salesforce_api.object_capabilities["Case"] = {
        "queryable": True,
        "createable": False,
        "updateable": True,
    }
    not_creatable = await create_case(
        _credentials(mock_salesforce_server), {"subject": "Nope"}, _config()
    )
    assert not_creatable.status_code == 403
    assert not mock_salesforce_api.created_records

    mock_salesforce_api.object_capabilities["Case"] = {
        "queryable": True,
        "createable": True,
        "updateable": False,
    }
    not_updatable = await update_case(
        _credentials(mock_salesforce_server),
        {"case_id": "500000000000001", "status": "Working"},
        _config(),
    )
    assert not_updatable.status_code == 403
    mock_salesforce_api.add_record(
        "QueueSobject",
        {
            "Id": "01I000000000001",
            "QueueId": "00G000000000001",
            "SobjectType": "Case",
        },
    )
    not_routable = await route_case(
        _credentials(mock_salesforce_server),
        {"case_id": "500000000000001", "queue_id": "00G000000000001"},
        _config(),
    )
    assert not_routable.status_code == 403
    assert not mock_salesforce_api.updated_records


async def test_queue_aggregate_counts_are_not_page_limited(
    mock_salesforce_api, mock_salesforce_server: str
) -> None:
    mock_salesforce_api.reset()
    mock_salesforce_api.add_group("00G000000000001", name="Support", group_type="Queue")
    mock_salesforce_api.add_record(
        "QueueSobject",
        {"Id": "QUE000000000001", "QueueId": "00G000000000001", "SobjectType": "Case"},
    )
    for index in range(60):
        mock_salesforce_api.add_record(
            "Case", _case_payload(record_id=f"50000000000{index:04d}", owner_id="00G000000000001")
        )
    response = await list_case_queues(_credentials(mock_salesforce_server), {}, _config())
    queue = json.loads(response.body)["result"]["queues"][0]
    assert queue["total_count"] == 60
    assert any("GROUP BY OwnerId, Status" in query for query in mock_salesforce_api.queries)


async def test_queue_discovery_pages_case_capable_rows_before_group_lookup(
    mock_salesforce_api, mock_salesforce_server: str
) -> None:
    mock_salesforce_api.reset()
    for index in range(55):
        queue_id = f"00G00000000{index:04d}"
        mock_salesforce_api.add_group(queue_id, name=f"Non-case {index}", group_type="Queue")
    case_queue_ids = [f"00G00000000{index:04d}" for index in range(100, 103)]
    for index, queue_id in enumerate(case_queue_ids):
        mock_salesforce_api.add_group(queue_id, name=f"Case {index}", group_type="Queue")
        mock_salesforce_api.add_record(
            "QueueSobject",
            {
                "Id": f"01I00000000{index:04d}",
                "QueueId": queue_id,
                "SobjectType": "Case",
            },
        )
    first = await list_case_queues(_credentials(mock_salesforce_server), {"limit": 2}, _config())
    first_result = json.loads(first.body)["result"]
    assert [queue["Id"] for queue in first_result["queues"]] == case_queue_ids[:2]
    assert first_result["next_cursor"]
    assert "FROM QueueSobject" in mock_salesforce_api.queries[0]
    assert "FROM Group WHERE Type = 'Queue' ORDER BY Id" not in mock_salesforce_api.queries
    second = await execute_action(
        "list_case_queues",
        {"limit": 2, "cursor": first_result["next_cursor"]},
        _credentials(mock_salesforce_server),
        source_config=_config(),
    )
    second_result = json.loads(second.body)["result"]
    assert [queue["Id"] for queue in second_result["queues"]] == case_queue_ids[2:]
    assert second_result["next_cursor"] is None


async def test_generic_case_polling_and_global_inbound_pagination(
    mock_salesforce_api, mock_salesforce_server: str
) -> None:
    mock_salesforce_api.reset()
    for index in range(60):
        case_id = f"50000000000{index:04d}"
        mock_salesforce_api.add_record("Case", _case_payload(record_id=case_id))
        mock_salesforce_api.add_record(
            "EmailMessage",
            {
                "Id": f"02s00000000{index:04d}",
                "ParentId": case_id,
                "FromAddress": "requester@example.com",
                "Incoming": True,
                "MessageDate": f"2024-06-01T10:{index // 60:02d}:{index % 60:02d}.000Z",
            },
        )
    first = await list_inbound_replies(
        _credentials(mock_salesforce_server),
        {"start_at": "2024-06-01T00:00:00Z", "end_at": "2024-06-02T00:00:00Z", "limit": 50},
        _config(),
    )
    result = json.loads(first.body)["result"]
    assert len(result["emails"]) == 50
    assert any(
        "ParentId IN (SELECT Id FROM Case WHERE Id != null)" in query
        for query in mock_salesforce_api.queries
    )
    second = await execute_action(
        "list_inbound_replies",
        {
            "start_at": "2024-06-01T00:00:00Z",
            "end_at": "2024-06-02T00:00:00Z",
            "limit": 50,
            "cursor": result["next_cursor"],
        },
        _credentials(mock_salesforce_server),
        source_config=_config(),
    )
    assert len(json.loads(second.body)["result"]["emails"]) == 10
    stale = await list_stale_cases(
        _credentials(mock_salesforce_server), {"stale_after_seconds": 0, "limit": 50}, _config()
    )
    assert len(json.loads(stale.body)["result"]["cases"]) == 50
    new = await list_new_cases(
        _credentials(mock_salesforce_server),
        {"start_at": "2023-01-01T00:00:00Z", "end_at": "2025-01-01T00:00:00Z", "limit": 50},
        _config(),
    )
    assert len(json.loads(new.body)["result"]["cases"]) == 50


async def test_case_detail_email_thread_and_polling_page_two(
    mock_salesforce_api, mock_salesforce_server: str
) -> None:
    mock_salesforce_api.reset()
    for index in range(60):
        case_id = f"50000000000{index:04d}"
        case = _case_payload(record_id=case_id)
        case["CreatedDate"] = f"2024-04-01T14:{index // 60:02d}:{index % 60:02d}.000Z"
        case["LastModifiedDate"] = f"2024-04-01T15:{index // 60:02d}:{index % 60:02d}.000Z"
        mock_salesforce_api.add_record("Case", case)
    detail = await get_case_detail(
        _credentials(mock_salesforce_server), {"case_id": "500000000000000"}, _config()
    )
    assert json.loads(detail.body)["result"]["id"] == "500000000000000"
    for index in range(60):
        mock_salesforce_api.add_record(
            "EmailMessage",
            {
                "Id": f"02s00000000{index:04d}",
                "ParentId": "500000000000000",
                "FromAddress": "requester@example.com",
                "Incoming": index % 2 == 0,
                "MessageDate": f"2024-06-01T10:{index // 60:02d}:{index % 60:02d}.000Z",
            },
        )
    thread = await get_case_email_thread(
        _credentials(mock_salesforce_server),
        {"case_id": "500000000000000", "limit": 50},
        _config(),
    )
    thread_result = json.loads(thread.body)["result"]
    assert len(thread_result["emails"]) == 50
    thread_page_two = await execute_action(
        "get_case_email_thread",
        {
            "case_id": "500000000000000",
            "limit": 50,
            "cursor": thread_result["next_cursor"],
        },
        _credentials(mock_salesforce_server),
        source_config=_config(),
    )
    assert len(json.loads(thread_page_two.body)["result"]["emails"]) == 10
    stale = await list_stale_cases(
        _credentials(mock_salesforce_server), {"stale_after_seconds": 0, "limit": 50}, _config()
    )
    stale_page = json.loads(stale.body)["result"]
    stale_two = await execute_action(
        "list_stale_cases",
        {"stale_after_seconds": 0, "limit": 50, "cursor": stale_page["next_cursor"]},
        _credentials(mock_salesforce_server),
        source_config=_config(),
    )
    assert len(json.loads(stale_two.body)["result"]["cases"]) == 10
    new = await list_new_cases(
        _credentials(mock_salesforce_server),
        {"start_at": "2024-04-01T00:00:00Z", "end_at": "2024-05-01T00:00:00Z", "limit": 50},
        _config(),
    )
    new_page = json.loads(new.body)["result"]
    new_two = await execute_action(
        "list_new_cases",
        {
            "start_at": "2024-04-01T00:00:00Z",
            "end_at": "2024-05-01T00:00:00Z",
            "limit": 50,
            "cursor": new_page["next_cursor"],
        },
        _credentials(mock_salesforce_server),
        source_config=_config(),
    )
    assert len(json.loads(new_two.body)["result"]["cases"]) == 10


async def test_requester_context_uses_optional_standard_fields_without_failing_hidden_fields(
    mock_salesforce_api, mock_salesforce_server: str
) -> None:
    mock_salesforce_api.reset()
    case = _case_payload()
    case["CreatedById"] = "005000000000001"
    mock_salesforce_api.add_record("Case", case)
    mock_salesforce_api.add_record("Contact", _contact_payload())
    mock_salesforce_api.add_record("Account", _account_payload())
    mock_salesforce_api.add_record("User", _user_payload())
    mock_salesforce_api.hidden_fields["Account"] = {"Industry"}
    mock_salesforce_api.hidden_fields["User"] = {"Title"}
    response = await get_case_requester_profile(
        _credentials(mock_salesforce_server), {"case_id": case["Id"]}, _config()
    )
    assert response.status_code == 200
    result = json.loads(response.body)["result"]
    assert set(result) == {"contact", "account", "creator"}


async def test_email_payload_and_typed_response_errors(
    mock_salesforce_api, mock_salesforce_server: str
) -> None:
    mock_salesforce_api.reset()
    mock_salesforce_api.add_record("Case", _case_payload())
    mock_salesforce_api.add_record(
        "EmailMessage",
        {
            "Id": "02s000000000001",
            "ParentId": "500000000000001",
            "FromAddress": "requester@example.com",
            "Incoming": True,
            "MessageDate": "2024-06-01T10:00:00.000Z",
        },
    )
    response = await reply_to_case(
        _credentials(mock_salesforce_server),
        {"case_id": "500000000000001", "body": "Reply"},
        _config(),
    )
    assert response.status_code == 200
    inputs = mock_salesforce_api.standard_action_requests[-1]["inputs"][0]
    assert inputs["senderType"] == "CurrentUser"
    assert inputs["logEmailOnSend"] is True
    assert inputs["addThreadingTokenToSubject"] is True
    assert inputs["addThreadingTokenToBody"] is True
    mock_salesforce_api.add_record(
        "ContentVersion",
        {
            "Id": "068000000000002",
            "ContentDocumentId": "069000000000002",
            "Title": "reply.txt",
            "FileExtension": "txt",
            "ContentSize": 4,
            "IsLatest": True,
        },
    )
    rejected_attachment = await reply_to_case(
        _credentials(mock_salesforce_server),
        {"case_id": "500000000000001", "body": "Unlinked", "file_ids": ["068000000000002"]},
        _config(),
    )
    assert rejected_attachment.status_code == 403
    assert len(mock_salesforce_api.standard_action_requests) == 1
    mock_salesforce_api.add_record(
        "ContentDocumentLink",
        {
            "Id": "06A000000000002",
            "ContentDocumentId": "069000000000002",
            "LinkedEntityId": "500000000000001",
        },
    )
    attached = await reply_to_case(
        _credentials(mock_salesforce_server),
        {"case_id": "500000000000001", "body": "With file", "file_ids": ["068000000000002"]},
        _config(),
    )
    assert attached.status_code == 200
    assert mock_salesforce_api.standard_action_requests[-1]["inputs"][0][
        "attachmentIdCollection"
    ] == ["068000000000002"]
    mock_salesforce_api.standard_action_5xx_remaining = 1
    ambiguous_email = await reply_to_case(
        _credentials(mock_salesforce_server),
        {"case_id": "500000000000001", "body": "Retry never"},
        _config(),
    )
    assert ambiguous_email.status_code == 502
    assert mock_salesforce_api.standard_action_5xx_remaining == 0
    mock_salesforce_api.standard_action_failure = True
    failed = await reply_to_case(
        _credentials(mock_salesforce_server),
        {"case_id": "500000000000001", "body": "Reply"},
        _config(),
    )
    assert failed.status_code == 502
    assert "INVALID_EMAIL_ADDRESS" in json.loads(failed.body)["error"]
    parsed = StandardActionResult.from_response(
        [{"isSuccess": True, "actionName": "emailSimple", "errors": None, "outputValues": None}]
    )[0]
    assert parsed.output_values is None
    with pytest.raises(SalesforceClientError):
        StandardActionResult.from_response(
            [
                {
                    "isSuccess": True,
                    "actionName": "emailSimple",
                    "errors": ["invalid"],
                    "outputValues": None,
                }
            ]
        )


async def test_files_are_paginated_and_authorized_after_many_links(
    mock_salesforce_api, mock_salesforce_server: str, monkeypatch: pytest.MonkeyPatch
) -> None:
    mock_salesforce_api.reset()
    mock_salesforce_api.add_record("Case", _case_payload())
    for index in range(55):
        document_id = f"06900000000{index:04d}"
        version_id = f"06800000000{index:04d}"
        mock_salesforce_api.add_record(
            "ContentDocumentLink",
            {
                "Id": f"06A00000000{index:04d}",
                "ContentDocumentId": document_id,
                "LinkedEntityId": "500000000000001",
            },
        )
        mock_salesforce_api.add_record(
            "ContentVersion",
            {
                "Id": version_id,
                "ContentDocumentId": document_id,
                "Title": f"file-{index}",
                "IsLatest": True,
            },
        )
    first = await list_case_files(
        _credentials(mock_salesforce_server), {"case_id": "500000000000001", "limit": 50}, _config()
    )
    page = json.loads(first.body)["result"]
    assert len(page["files"]) == 50 and page["next_cursor"]
    second = await execute_action(
        "list_case_files",
        {"case_id": "500000000000001", "limit": 50, "cursor": page["next_cursor"]},
        _credentials(mock_salesforce_server),
        source_config=_config(),
    )
    assert len(json.loads(second.body)["result"]["files"]) == 5
    assert (
        await fetch_case_file(
            _credentials(mock_salesforce_server),
            {"case_id": "500000000000001", "content_version_id": "068000000000054"},
            _config(),
        )
    ).status_code == 200
    mock_salesforce_api.add_record(
        "ContentVersion",
        {
            "Id": "068000000000055",
            "ContentDocumentId": "069000000000055",
            "Title": "unlinked.txt",
            "ContentSize": 1,
            "IsLatest": True,
        },
    )
    unlinked = await fetch_case_file(
        _credentials(mock_salesforce_server),
        {"case_id": "500000000000001", "content_version_id": "068000000000055"},
        _config(),
    )
    assert unlinked.status_code == 403
    mock_salesforce_api.add_record(
        "ContentVersion",
        {
            "Id": "068000000000056",
            "ContentDocumentId": "069000000000056",
            "Title": "declared-large.bin",
            "ContentSize": case_actions.MAX_FILE_BYTES + 1,
            "IsLatest": True,
        },
    )
    mock_salesforce_api.add_record(
        "ContentDocumentLink",
        {
            "Id": "06A000000000056",
            "ContentDocumentId": "069000000000056",
            "LinkedEntityId": "500000000000001",
        },
    )
    declared_large = await fetch_case_file(
        _credentials(mock_salesforce_server),
        {"case_id": "500000000000001", "content_version_id": "068000000000056"},
        _config(),
    )
    assert declared_large.status_code == 400
    monkeypatch.setattr(case_actions, "MAX_FILE_BYTES", 8)
    mock_salesforce_api.add_record(
        "ContentVersion",
        {
            "Id": "068000000000057",
            "ContentDocumentId": "069000000000057",
            "Title": "stream-large.bin",
            "ContentSize": 1,
            "IsLatest": True,
        },
    )
    mock_salesforce_api.add_record(
        "ContentDocumentLink",
        {
            "Id": "06A000000000057",
            "ContentDocumentId": "069000000000057",
            "LinkedEntityId": "500000000000001",
        },
    )
    mock_salesforce_api.binary_files["068000000000057"] = b"123456789"
    streamed_large = await fetch_case_file(
        _credentials(mock_salesforce_server),
        {"case_id": "500000000000001", "content_version_id": "068000000000057"},
        _config(),
    )
    assert streamed_large.status_code == 502


def test_describe_requires_typed_object_capabilities() -> None:
    described = ObjectDescribe.from_response(
        {"queryable": True, "createable": False, "updateable": True, "fields": []}
    )
    assert described.queryable is True
    assert described.createable is False
    with pytest.raises(SalesforceClientError, match="queryable expected boolean"):
        ObjectDescribe.from_response({"fields": []})


def test_generic_manifest_and_config_have_no_policy_surface() -> None:
    config = SalesforceSourceConfig.from_mapping(
        {"obsolete_case_settings": {"record_type_id": "012000000000001"}}
    )
    assert config == SalesforceSourceConfig()
    actions = SalesforceConnector().actions
    names = {action.name for action in actions}
    assert "mark_case_reviewed" not in names
    assert all("custom_fields" not in str(action.input_schema) for action in actions)
