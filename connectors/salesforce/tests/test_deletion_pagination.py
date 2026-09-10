"""Unit tests for Salesforce ``/deleted`` pagination URL normalization.

``simple_salesforce.restful`` already prepends the versioned API base path, so
a raw ``nextRecordsUrl`` must be reduced to its relative path exactly once.
Passing it through unchanged would produce ``/services/data/v62.0/services/...``
and fail.
"""

from __future__ import annotations

from collections.abc import Mapping
from datetime import UTC, datetime, timedelta

import pytest

from salesforce_connector.client import (
    DeletedResult,
    SalesforceClient,
    SalesforceClientError,
    _normalize_next_records_path,
)
from salesforce_connector.models import SalesforceAuth
from tests.conftest import MockSalesforceAPI

_VALID_NEXT_URL = (
    "https://acme.my.salesforce.com/services/data/v62.0/sobjects/Account/deleted?queryId=2"
)


def test_normalize_strips_the_api_prefix_exactly_once() -> None:
    path, params = _normalize_next_records_path(_VALID_NEXT_URL)
    assert path == "sobjects/Account/deleted"
    assert params == {"queryId": "2"}


@pytest.mark.parametrize(
    "url",
    [
        "https://acme.my.salesforce.com/services/data/v61.0/sobjects/Account/deleted",
        "https://acme.my.salesforce.com/services/data/v62.0/",
        "https://acme.my.salesforce.com/services/data/v62.0/../etc/passwd",
        "https://acme.my.salesforce.com/services/data/v62.0/sobjects/Account/deleted/../../x",
        "https://acme.my.salesforce.com/services/data/v62.0//absolute",
        "https://acme.my.salesforce.com/services/data/v62.0/sobjects/Account/deleted/",
        "not-a-salesforce-url",
    ],
)
def test_normalize_rejects_invalid_paths(url: str) -> None:
    with pytest.raises(SalesforceClientError):
        _normalize_next_records_path(url)


class _RecordingSession:
    def __init__(self) -> None:
        self.calls: list[tuple[str, Mapping[str, str] | None]] = []

    def restful(
        self, path: str, params: Mapping[str, str] | None = None
    ) -> Mapping[str, object]:
        self.calls.append((path, params))
        return {
            "deletedRecords": [],
            "earliestDateAvailable": None,
            "latestDateCovered": None,
        }


async def test_get_deleted_more_calls_restful_with_the_relative_path(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    client = SalesforceClient(
        SalesforceAuth.from_mapping(
            {"access_token": "t", "instance_url": "https://acme.my.salesforce.com"}
        )
    )
    session = _RecordingSession()

    async def fake_ensure(_self: SalesforceClient) -> object:
        return session

    monkeypatch.setattr(SalesforceClient, "_ensure_session", fake_ensure)
    result = await client.get_deleted_more(_VALID_NEXT_URL)

    assert result.deleted_records == ()
    assert session.calls == [("sobjects/Account/deleted", {"queryId": "2"})]


async def test_get_deleted_more_rejects_invalid_paths_before_calling_the_api() -> None:
    client = SalesforceClient(
        SalesforceAuth.from_mapping(
            {"access_token": "t", "instance_url": "https://acme.my.salesforce.com"}
        )
    )
    with pytest.raises(SalesforceClientError):
        await client.get_deleted_more(
            "https://acme.my.salesforce.com/services/data/v61.0/sobjects/Account/deleted"
        )


async def test_get_deleted_more_follows_a_salesforce_url_end_to_end(
    mock_salesforce_api: MockSalesforceAPI, mock_salesforce_server: str
) -> None:
    mock_salesforce_api.reset()
    for index in range(3):
        record_id = f"00100000000000{index}"
        mock_salesforce_api.add_account(record_id)
        mock_salesforce_api.mark_deleted("Account", record_id)
    mock_salesforce_api.deleted_page_size = 1

    client = SalesforceClient(
        SalesforceAuth.from_mapping(
            {"access_token": "test-token", "instance_url": mock_salesforce_server}
        )
    )
    start = datetime.now(UTC) - timedelta(days=1)
    end = datetime.now(UTC) + timedelta(days=1)
    first: DeletedResult = await client.get_deleted("Account", start, end)
    assert len(first.deleted_records) == 1
    assert first.next_records_url is not None

    second = await client.get_deleted_more(first.next_records_url)
    assert len(second.deleted_records) == 1
    assert second.next_records_url is not None

    third = await client.get_deleted_more(second.next_records_url)
    assert len(third.deleted_records) == 1
    assert third.next_records_url is None
