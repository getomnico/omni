"""Tests for Microsoft connector calendar actions."""

import json
from datetime import datetime, timezone

import pytest

from ms_connector import MicrosoftConnector
from omni_connector.models import Source

from .conftest import MockGraphAPI


def _source(graph_base_url: str) -> Source:
    return Source(
        id="src-calendar-1",
        name="Outlook Calendar",
        source_type="outlook_calendar",
        config={"graph_base_url": graph_base_url},
        is_active=True,
        is_deleted=False,
        scope="org",
        created_by="test-user",
        created_at=datetime.now(timezone.utc),
        updated_at=datetime.now(timezone.utc),
    )


def _envelope() -> dict:
    return {
        "credentials": {"token": "test-token"},
        "config": {},
        "source_id": "src-calendar-1",
        "provider": "microsoft",
    }


def _graph_event(
    subject: str,
    start: str,
    end: str,
    location: str = "Room 1",
) -> dict:
    return {
        "subject": subject,
        "start": {"dateTime": start, "timeZone": "UTC"},
        "end": {"dateTime": end, "timeZone": "UTC"},
        "location": {"displayName": location},
        "organizer": {
            "emailAddress": {"name": "Alice", "address": "alice@example.com"}
        },
        "attendees": [
            {"emailAddress": {"name": "Bob", "address": "bob@example.com"}}
        ],
        "webLink": "https://outlook.example.com/calendar/item",
        "isAllDay": False,
        "isCancelled": False,
    }


def _actions_by_name() -> dict:
    return {a.name: a for a in MicrosoftConnector().actions}


def test_calendar_actions_declared_for_outlook_calendar_source():
    actions = _actions_by_name()

    list_events = actions["list_events"]
    assert list_events.mode == "read"
    assert list_events.source_types == ["outlook_calendar"]
    assert list_events.required_scopes == ["Calendars.Read"]
    assert not {"anyOf", "oneOf", "allOf"} & set(list_events.input_schema)

    create_event = actions["create_event"]
    assert create_event.mode == "write"
    assert create_event.source_types == ["outlook_calendar"]
    assert create_event.required_scopes == ["Calendars.ReadWrite"]
    assert set(create_event.input_schema["required"]) == {"subject", "start", "end"}


@pytest.mark.asyncio
async def test_list_events_returns_events_in_range_ordered(
    mock_graph_api: MockGraphAPI,
    mock_graph_server: str,
):
    mock_graph_api.reset()
    mock_graph_api.add_me_event(
        _graph_event("Standup", "2026-09-16T09:00:00", "2026-09-16T09:30:00")
    )
    mock_graph_api.add_me_event(
        _graph_event("Retro", "2026-09-16T15:00:00", "2026-09-16T16:00:00", "Room 2")
    )
    mock_graph_api.add_me_event(
        _graph_event("Outside", "2026-09-20T09:00:00", "2026-09-20T10:00:00")
    )

    response = await MicrosoftConnector().execute_action(
        "list_events",
        {"start": "2026-09-16T00:00:00Z", "end": "2026-09-17T00:00:00Z"},
        _envelope(),
        source=_source(f"{mock_graph_server}/v1.0"),
    )

    assert response.status_code == 200
    payload = json.loads(response.body)
    assert payload["status"] == "success"
    result = payload["result"]
    assert result["count"] == 2
    assert [e["subject"] for e in result["events"]] == ["Standup", "Retro"]
    assert result["range"] == {
        "start": "2026-09-16T00:00:00+00:00",
        "end": "2026-09-17T00:00:00+00:00",
    }
    standup = result["events"][0]
    assert standup["location"] == "Room 1"
    assert standup["organizer"] == "alice@example.com"
    assert standup["attendees"] == ["bob@example.com"]
    assert standup["is_all_day"] is False
    assert standup["id"]


@pytest.mark.asyncio
async def test_list_events_respects_top(
    mock_graph_api: MockGraphAPI,
    mock_graph_server: str,
):
    mock_graph_api.reset()
    mock_graph_api.add_me_event(
        _graph_event("One", "2026-09-16T09:00:00", "2026-09-16T09:30:00")
    )
    mock_graph_api.add_me_event(
        _graph_event("Two", "2026-09-16T10:00:00", "2026-09-16T10:30:00")
    )

    response = await MicrosoftConnector().execute_action(
        "list_events",
        {
            "start": "2026-09-16T00:00:00Z",
            "end": "2026-09-17T00:00:00Z",
            "top": 1,
        },
        _envelope(),
        source=_source(f"{mock_graph_server}/v1.0"),
    )

    payload = json.loads(response.body)
    assert payload["result"]["count"] == 1
    assert payload["result"]["events"][0]["subject"] == "One"


@pytest.mark.asyncio
async def test_list_events_defaults_to_next_seven_days(
    mock_graph_api: MockGraphAPI,
    mock_graph_server: str,
):
    mock_graph_api.reset()
    response = await MicrosoftConnector().execute_action(
        "list_events",
        {},
        _envelope(),
        source=_source(f"{mock_graph_server}/v1.0"),
    )

    assert response.status_code == 200
    payload = json.loads(response.body)
    assert payload["result"]["events"] == []


@pytest.mark.asyncio
async def test_list_events_rejects_invalid_ranges(
    mock_graph_api: MockGraphAPI,
    mock_graph_server: str,
):
    connector = MicrosoftConnector()
    envelope = _envelope()
    source = _source(f"{mock_graph_server}/v1.0")

    response = await connector.execute_action(
        "list_events",
        {"start": "2026-09-17T00:00:00Z", "end": "2026-09-16T00:00:00Z"},
        envelope,
        source=source,
    )
    assert response.status_code == 400

    response = await connector.execute_action(
        "list_events",
        {"start": "not-a-date"},
        envelope,
        source=source,
    )
    assert response.status_code == 400
    assert "Invalid start" in json.loads(response.body)["error"]


@pytest.mark.asyncio
async def test_create_event_posts_utc_payload(
    mock_graph_api: MockGraphAPI,
    mock_graph_server: str,
):
    mock_graph_api.reset()
    response = await MicrosoftConnector().execute_action(
        "create_event",
        {
            "subject": "Design review",
            "start": "2026-09-17T14:00:00+02:00",
            "end": "2026-09-17T15:00:00+02:00",
            "attendees": ["bob@example.com"],
            "location": "Room 3",
            "body": "Bring the specs",
            "online_meeting": True,
        },
        _envelope(),
        source=_source(f"{mock_graph_server}/v1.0"),
    )

    assert response.status_code == 200
    payload = json.loads(response.body)
    event = payload["result"]["event"]
    assert event["subject"] == "Design review"
    assert event["id"].startswith("me-event-")
    assert event["location"] == "Room 3"
    assert event["body_preview"] == "Bring the specs"

    stored = mock_graph_api.me_events[0]
    assert stored["start"] == {"dateTime": "2026-09-17T12:00:00", "timeZone": "UTC"}
    assert stored["end"] == {"dateTime": "2026-09-17T13:00:00", "timeZone": "UTC"}
    assert stored["isOnlineMeeting"] is True
    assert stored["attendees"] == [
        {"emailAddress": {"address": "bob@example.com"}, "type": "required"}
    ]


@pytest.mark.asyncio
async def test_create_event_validates_input(
    mock_graph_api: MockGraphAPI,
    mock_graph_server: str,
):
    mock_graph_api.reset()
    connector = MicrosoftConnector()
    envelope = _envelope()
    source = _source(f"{mock_graph_server}/v1.0")

    response = await connector.execute_action(
        "create_event",
        {"start": "2026-09-17T14:00:00Z", "end": "2026-09-17T15:00:00Z"},
        envelope,
        source=source,
    )
    assert response.status_code == 400

    response = await connector.execute_action(
        "create_event",
        {
            "subject": "Broken",
            "start": "2026-09-17T14:00:00Z",
            "end": "2026-09-17T13:00:00Z",
        },
        envelope,
        source=source,
    )
    assert response.status_code == 400

    response = await connector.execute_action(
        "create_event",
        {
            "subject": "Broken",
            "start": "2026-09-17T14:00:00Z",
            "end": "2026-09-17T15:00:00Z",
            "attendees": ["not-an-email"],
        },
        envelope,
        source=source,
    )
    assert response.status_code == 400
    assert mock_graph_api.me_events == []


@pytest.mark.asyncio
async def test_list_events_accepts_refresh_fallback_credential_payload(
    mock_graph_api: MockGraphAPI,
    mock_graph_server: str,
):
    """The web OAuth callback stores client/endpoint metadata next to the
    tokens so connector-manager can refresh them; actions must accept it."""
    mock_graph_api.reset()
    mock_graph_api.add_me_event(
        _graph_event("Sync check", "2026-09-16T11:00:00", "2026-09-16T11:30:00")
    )

    envelope = {
        "credentials": {
            "access_token": "test-token",
            "refresh_token": "0.AYEA_refresh",
            "token_type": "Bearer",
            "client_id": "6021d430-1f5f-446d-ab7a-8d4e66cb6b88",
            "client_secret": "secret-value",
            "token_uri": f"https://login.microsoftonline.com/tenant/oauth2/v2.0/token",
            "token_endpoint_auth_method": "client_secret_post",
            "scope": "Calendars.Read User.Read",
        },
        "config": {},
        "source_id": "src-calendar-1",
        "provider": "microsoft",
    }

    response = await MicrosoftConnector().execute_action(
        "list_events",
        {"start": "2026-09-16T00:00:00Z", "end": "2026-09-17T00:00:00Z"},
        envelope,
        source=_source(f"{mock_graph_server}/v1.0"),
    )

    assert response.status_code == 200
    payload = json.loads(response.body)
    assert payload["result"]["count"] == 1
    assert payload["result"]["events"][0]["subject"] == "Sync check"


@pytest.mark.asyncio
async def test_list_events_rejects_unrecognized_credential_shape(
    mock_graph_api: MockGraphAPI,
    mock_graph_server: str,
):
    mock_graph_api.reset()
    envelope = {
        "credentials": {"access_token": "tok", "unexpected_field": True},
        "config": {},
    }

    response = await MicrosoftConnector().execute_action(
        "list_events",
        {},
        envelope,
        source=_source(f"{mock_graph_server}/v1.0"),
    )

    assert response.status_code == 400
    assert "Unrecognized Microsoft credential shape" in json.loads(response.body)[
        "error"
    ]
