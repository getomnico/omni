"""Unit tests for client retry bounds and wire-response validation."""

from __future__ import annotations

from typing import Any

import pytest

import salesforce_connector.client as client_module
from salesforce_connector.client import SalesforceClient, SalesforceClientError
from salesforce_connector.models import SalesforceAuth
from tests.conftest import MockSalesforceAPI


def _client(mock_server: str) -> SalesforceClient:
    return SalesforceClient(
        SalesforceAuth.from_mapping(
            {"access_token": "test-token", "instance_url": mock_server}
        )
    )


async def test_rate_limit_retries_are_bounded(
    mock_salesforce_api: MockSalesforceAPI,
    mock_salesforce_server: str,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    mock_salesforce_api.reset()
    mock_salesforce_api.add_account()
    mock_salesforce_api.rate_limit_remaining = 100
    monkeypatch.setattr(client_module, "RATE_LIMIT_MAX_RETRIES", 2)
    monkeypatch.setattr(client_module, "RATE_LIMIT_BASE_DELAY_SECONDS", 0.0)

    client = _client(mock_salesforce_server)
    with pytest.raises(SalesforceClientError, match="Rate limited"):
        await client.query("SELECT Id FROM Account")

    # The initial call plus exactly the bounded number of retries.
    assert mock_salesforce_api.rate_limit_hits == 3


async def test_rate_limit_recovers_within_bound(
    mock_salesforce_api: MockSalesforceAPI,
    mock_salesforce_server: str,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    mock_salesforce_api.reset()
    mock_salesforce_api.add_account()
    mock_salesforce_api.rate_limit_remaining = 1
    monkeypatch.setattr(client_module, "RATE_LIMIT_MAX_RETRIES", 2)
    monkeypatch.setattr(client_module, "RATE_LIMIT_BASE_DELAY_SECONDS", 0.0)

    result = await _client(mock_salesforce_server).query("SELECT Id FROM Account")
    assert result.total_size == 1


async def test_jwt_token_response_must_be_a_mapping(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    class FakeResponse:
        status_code = 200
        text = "[]"

        @staticmethod
        def json() -> Any:
            return ["not", "a", "mapping"]

    monkeypatch.setattr(client_module.requests, "post", lambda *a, **k: FakeResponse())
    monkeypatch.setattr(client_module.jwt, "encode", lambda *a, **k: "assertion")

    auth = SalesforceAuth.from_mapping(
        {
            "client_id": "cid",
            "private_key": "pem",
            "username": "u@example.com",
        }
    )
    client = SalesforceClient(auth)
    with pytest.raises(SalesforceClientError, match="malformed JWT token response"):
        await client.session_credentials()
