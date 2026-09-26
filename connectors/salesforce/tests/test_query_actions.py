from __future__ import annotations

import json
from collections.abc import Mapping
from typing import Any

import pytest
from fastapi.responses import JSONResponse

import salesforce_connector.query_actions as query_actions
from salesforce_connector.client import (
    AuthenticationError,
    QueryResult,
    SalesforceUserIdentity,
)
from salesforce_connector.models import SalesforceSourceConfig


class FakeClient:
    def __init__(self) -> None:
        self.queries: list[tuple[str, bool]] = []
        self.pages = [
            QueryResult(False, 2, ({"Id": "one"},), "/page-2"),
            QueryResult(True, 2, ({"Id": "two"},), None),
        ]

    async def query(self, query: str) -> QueryResult:
        self.queries.append((query, False))
        return self.pages[0]

    async def query_tooling(self, query: str) -> QueryResult:
        self.queries.append((query, True))
        return self.pages[0]

    async def query_more(self, url: str) -> QueryResult:
        assert url == "/page-2"
        return self.pages[1]

    async def query_more_tooling(self, url: str) -> QueryResult:
        assert url == "/page-2"
        return self.pages[1]


@pytest.fixture
def fake_salesforce(monkeypatch: pytest.MonkeyPatch) -> FakeClient:
    client = FakeClient()
    identity = SalesforceUserIdentity(
        "00D000000000001",
        "005000000000001",
        "person@example.com",
        "https://acme.my.salesforce.com",
    )
    monkeypatch.setattr(query_actions, "fetch_user_identity", _identity(identity))
    monkeypatch.setattr(query_actions, "SalesforceClient", lambda *args, **kwargs: client)
    return client


def _identity(identity: SalesforceUserIdentity) -> Any:
    async def get_identity(_: object) -> SalesforceUserIdentity:
        return identity

    return get_identity


def _response_data(response: JSONResponse) -> Mapping[str, object]:
    return json.loads(response.body)


def _params(**overrides: object) -> dict[str, object]:
    return {
        "query": "SELECT Id FROM Account",
        "usernameOrAlias": "person@example.com",
        "directory": "/not-used-by-native-actions",
        **overrides,
    }


def _source() -> dict[str, object]:
    return {
        "id": "source-1",
        "source_type": "salesforce",
        "source_binding": {"organization_id": "00D000000000001"},
    }


def _credentials() -> dict[str, object]:
    return {"access_token": "secret-token", "instance_url": "https://acme.my.salesforce.com"}


@pytest.mark.asyncio
async def test_soql_action_contract_and_follows_pages(fake_salesforce: FakeClient) -> None:
    result = await query_actions.execute_query_action(
        "run_soql_query", _params(), _credentials(), SalesforceSourceConfig(), _source()
    )
    assert isinstance(result, JSONResponse)
    body = _response_data(result)
    text = body["result"]["content"]
    assert isinstance(text, str) and text.startswith("SOQL query results:\n\n")
    envelope = json.loads(text.split("\n\n", 1)[1])
    assert envelope == {"totalSize": 2, "done": True, "records": [{"Id": "one"}, {"Id": "two"}]}
    assert fake_salesforce.queries == [("SELECT Id FROM Account", False)]


@pytest.mark.asyncio
async def test_get_username_preserves_mcp_content_result_key(
    fake_salesforce: FakeClient,
) -> None:
    result = await query_actions.execute_query_action(
        "get_username",
        {"directory": "/unused"},
        _credentials(),
        SalesforceSourceConfig(),
        _source(),
    )
    assert isinstance(result, JSONResponse)
    body = _response_data(result)
    assert "person@example.com" in body["result"]["content"]
    assert "text" not in body["result"]


@pytest.mark.asyncio
async def test_tooling_api_uses_tooling_pagination(fake_salesforce: FakeClient) -> None:
    result = await query_actions.execute_query_action(
        "run_soql_query",
        _params(useToolingApi=True),
        _credentials(),
        SalesforceSourceConfig(),
        _source(),
    )
    assert isinstance(result, JSONResponse)
    assert result.status_code == 200
    assert fake_salesforce.queries == [("SELECT Id FROM Account", True)]


@pytest.mark.asyncio
async def test_user_identity_uses_provider_verified_rest_host_and_rejects_unsafe_host(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from types import SimpleNamespace

    from salesforce_connector import client as client_module
    from salesforce_connector.client import fetch_user_identity
    from salesforce_connector.models import SalesforceAuth

    requested: list[str] = []
    sobjects_url = (
        "https://provider.my.salesforce.com/services/data/"
        f"{client_module.API_VERSION}/sobjects"
    )

    def get(url: str, **kwargs: object) -> SimpleNamespace:
        requested.append(url)
        if url.endswith("/services/oauth2/userinfo"):
            return SimpleNamespace(
                status_code=200,
                json=lambda: {
                    "organization_id": "00D000000000001",
                    "user_id": "005000000000001",
                    "preferred_username": "person@example.com",
                },
            )
        return SimpleNamespace(
            status_code=200,
            json=lambda: {"sobjects": sobjects_url},
        )

    monkeypatch.setattr(client_module.requests, "get", get)
    auth = SalesforceAuth.from_mapping(
        {
            "access_token": "token",
            "instance_url": "https://claimed.my.salesforce.com",
            "login_url": "https://login.salesforce.com",
        }
    )
    identity = await fetch_user_identity(auth)
    assert identity.instance_url == "https://provider.my.salesforce.com"
    assert requested == [
        "https://login.salesforce.com/services/oauth2/userinfo",
        f"https://claimed.my.salesforce.com/services/data/{client_module.API_VERSION}/",
    ]

    sobjects_url = f"/services/data/{client_module.API_VERSION}/sobjects"
    requested.clear()
    identity = await fetch_user_identity(auth)
    assert identity.instance_url == "https://claimed.my.salesforce.com"

    requested.clear()
    unsafe_auth = SalesforceAuth.from_mapping(
        {"access_token": "token", "instance_url": "https://127.0.0.1"}
    )
    with pytest.raises(client_module.SalesforceClientError):
        await fetch_user_identity(unsafe_auth)
    assert requested == []


@pytest.mark.parametrize(
    "query",
    [
        "DELETE FROM Account",
        "SELECT Id FROM Account; DELETE FROM Account",
        "SELECT Id FROM Account --comment",
        "SELECT Id FROM Account FOR UPDATE",
        "SELECT Id FROM Account /* comment */",
    ],
)
def test_rejects_mutating_or_multistatement_soql(query: str) -> None:
    with pytest.raises(ValueError):
        query_actions._validate_read_query(query)


def test_quoted_mutation_words_are_not_treated_as_executable_tokens() -> None:
    assert query_actions._validate_read_query("SELECT Id FROM Account WHERE Name = 'delete'")


@pytest.mark.asyncio
async def test_wrong_source_org_and_wrong_user_fail_closed(fake_salesforce: FakeClient) -> None:
    wrong_org = {"source_binding": {"organization_id": "00D000000000002"}}
    response = await query_actions.execute_query_action(
        "run_soql_query", _params(), _credentials(), SalesforceSourceConfig(), wrong_org
    )
    assert isinstance(response, JSONResponse) and response.status_code == 403
    response = await query_actions.execute_query_action(
        "run_soql_query",
        _params(usernameOrAlias="other@example.com"),
        _credentials(),
        SalesforceSourceConfig(),
        _source(),
    )
    assert isinstance(response, JSONResponse) and response.status_code == 400
    assert fake_salesforce.queries == []


@pytest.mark.asyncio
async def test_untrusted_login_url_and_unverified_instance_are_rejected_before_query(
    fake_salesforce: FakeClient,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    async def should_not_fetch(_: object) -> SalesforceUserIdentity:
        raise AssertionError("unsafe login URL reached identity endpoint")

    monkeypatch.setattr(query_actions, "fetch_user_identity", should_not_fetch)
    response = await query_actions.execute_query_action(
        "run_soql_query",
        _params(),
        _credentials(),
        SalesforceSourceConfig(),
        {**_source(), "login_url": "http://127.0.0.1:8080"},
    )
    assert isinstance(response, JSONResponse) and response.status_code == 400

    verified_elsewhere = SalesforceUserIdentity(
        "00D000000000001",
        "005000000000001",
        "person@example.com",
        "https://attacker.my.salesforce.com",
    )
    monkeypatch.setattr(query_actions, "fetch_user_identity", _identity(verified_elsewhere))
    response = await query_actions.execute_query_action(
        "run_soql_query", _params(), _credentials(), SalesforceSourceConfig(), _source()
    )
    assert isinstance(response, JSONResponse) and response.status_code == 403
    assert fake_salesforce.queries == []


@pytest.mark.asyncio
async def test_missing_source_binding_and_jwt_never_fall_back_to_org(
    fake_salesforce: FakeClient,
) -> None:
    response = await query_actions.execute_query_action(
        "run_soql_query", _params(), _credentials(), SalesforceSourceConfig(), {}
    )
    assert isinstance(response, JSONResponse) and response.status_code == 403
    response = await query_actions.execute_query_action(
        "run_soql_query",
        _params(),
        {"client_id": "client", "private_key": "key", "username": "org-user"},
        SalesforceSourceConfig(),
        _source(),
    )
    assert isinstance(response, JSONResponse) and response.status_code == 400
    assert fake_salesforce.queries == []


def test_only_replacement_actions_are_user_read_actions() -> None:
    defs = {definition.name: definition for definition in query_actions.QUERY_ACTION_DEFINITIONS}
    assert set(defs) == {"run_soql_query", "get_username"}
    assert all(definition.mode == "read" for definition in defs.values())
    assert all(definition.credential_scope == "user" for definition in defs.values())
    assert "list_all_orgs" not in defs


def test_action_http_endpoint_rejects_bad_owner_and_returns_reconnect_on_expiry(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from fastapi.testclient import TestClient
    from omni_connector.config import SdkConfig
    from omni_connector.server import create_app

    from salesforce_connector.connector import SalesforceConnector

    async def no_identity(_: object) -> SalesforceUserIdentity:
        raise AssertionError("identity must not be fetched for an invalid credential envelope")

    monkeypatch.setattr(query_actions, "fetch_user_identity", no_identity)
    app = create_app(
        SalesforceConnector(),
        config=SdkConfig(
            connector_manager_url="http://localhost:9000", connector_host_name="localhost"
        ),
    )
    source = {
        "id": "source-1",
        "name": "Salesforce",
        "source_type": "salesforce",
        "config": {"source_binding": {"organization_id": "00D000000000001"}},
        "is_active": True,
        "is_deleted": False,
        "scope": "org",
        "created_at": "2025-01-01T00:00:00Z",
        "updated_at": "2025-01-01T00:00:00Z",
        "created_by": "creator-1",
    }
    payload = {
        "action": "run_soql_query",
        "params": _params(),
        "credentials": {
            "source_id": "source-1",
            "user_id": "other-user",
            "credentials": _credentials(),
        },
        "source": source,
        "actor_email": "actor@example.com",
        "actor_user_id": "actor-1",
    }
    with TestClient(app) as http:
        response = http.post("/action", json=payload)
        assert response.status_code == 403
        payload["credentials"]["source_id"] = "another-source"
        payload["credentials"]["user_id"] = "actor-1"
        response = http.post("/action", json=payload)
        assert response.status_code == 403
        payload["credentials"]["source_id"] = "source-1"
        payload.pop("actor_user_id")
        response = http.post("/action", json=payload)
        assert response.status_code == 403

        async def expired(_: object) -> SalesforceUserIdentity:
            raise AuthenticationError("private provider detail")

        monkeypatch.setattr(query_actions, "fetch_user_identity", expired)
        payload["actor_user_id"] = "actor-1"
        response = http.post("/action", json=payload)
        assert response.status_code == 412, response.text
        assert response.json() == {
            "error": "needs_user_auth",
            "source_id": "source-1",
            "source_type": "salesforce",
            "provider": "salesforce",
            "oauth_start_url": "/api/oauth/start?source_id=source-1",
        }


@pytest.mark.asyncio
async def test_large_results_over_one_megabyte_remain_available_for_ai_artifact_handling(
    fake_salesforce: FakeClient,
) -> None:
    fake_salesforce.pages = [
        QueryResult(True, 1, ({"Id": "large", "value": "x" * 1_100_000},), None)
    ]
    result = await query_actions.execute_query_action(
        "run_soql_query", _params(), _credentials(), SalesforceSourceConfig(), _source()
    )
    assert isinstance(result, JSONResponse)
    assert result.status_code == 200
    text = _response_data(result)["result"]["content"]
    assert isinstance(text, str) and len(text.encode("utf-8")) > 1_000_000


@pytest.mark.asyncio
async def test_large_result_is_rejected_without_creating_artifact(
    fake_salesforce: FakeClient,
    monkeypatch: pytest.MonkeyPatch,
    tmp_path,
) -> None:
    monkeypatch.setattr(query_actions, "MAX_RESULT_BYTES", 20)
    result = await query_actions.execute_query_action(
        "run_soql_query", _params(), _credentials(), SalesforceSourceConfig(), _source()
    )
    assert isinstance(result, JSONResponse)
    assert result.status_code == 502
    assert list(tmp_path.iterdir()) == []


@pytest.mark.asyncio
async def test_expired_oauth_uses_needs_user_auth_contract(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from salesforce_connector.client import AuthenticationError

    async def expired(_: object) -> SalesforceUserIdentity:
        raise AuthenticationError("secret and provider detail must not escape")

    monkeypatch.setattr(query_actions, "fetch_user_identity", expired)
    result = await query_actions.execute_query_action(
        "run_soql_query", _params(), _credentials(), SalesforceSourceConfig(), _source()
    )
    assert isinstance(result, JSONResponse)
    assert result.status_code == 412
    assert _response_data(result) == {
        "error": "needs_user_auth",
        "source_id": "source-1",
        "source_type": "salesforce",
        "provider": "salesforce",
        "oauth_start_url": "/api/oauth/start?source_id=source-1",
    }


@pytest.mark.asyncio
async def test_provider_errors_do_not_expose_query_or_provider_message(
    fake_salesforce: FakeClient,
) -> None:
    from salesforce_connector.client import SalesforceClientError

    async def provider_error(_: str) -> QueryResult:
        raise SalesforceClientError("secret query, record data, and token")

    fake_salesforce.query = provider_error  # type: ignore[method-assign]
    result = await query_actions.execute_query_action(
        "run_soql_query", _params(), _credentials(), SalesforceSourceConfig(), _source()
    )
    assert isinstance(result, JSONResponse)
    body = result.body.decode()
    assert result.status_code == 502
    assert "secret" not in body
    assert "SELECT" not in body
