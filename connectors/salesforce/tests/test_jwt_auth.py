"""Unit tests: JWT bearer auth — credential parsing, token acquisition, self-refresh."""

from __future__ import annotations

import jwt
import pytest
from cryptography.hazmat.primitives import serialization
from cryptography.hazmat.primitives.asymmetric import rsa

from salesforce_connector.client import (
    AuthenticationError,
    SalesforceClient,
    fetch_organization_id,
)
from salesforce_connector.connector import SalesforceConnector
from salesforce_connector.models import AuthMode, SalesforceAuth
from tests.conftest import MockSalesforceAPI

pytestmark = pytest.mark.usefixtures("mock_salesforce_api")

_private_key = (
    rsa.generate_private_key(public_exponent=65537, key_size=2048)
    .private_bytes(
        serialization.Encoding.PEM,
        serialization.PrivateFormat.PKCS8,
        serialization.NoEncryption(),
    )
    .decode()
)


def _jwt_credentials(mock_server: str) -> dict[str, str]:
    return {
        "client_id": "3MVG9-test-consumer-key",
        "private_key": _private_key,
        "username": "owner@example.com",
        "login_url": mock_server,
        "instance_url": mock_server,
    }


# --- SalesforceAuth.from_mapping -------------------------------------------------


def test_from_mapping_bearer() -> None:
    auth = SalesforceAuth.from_mapping({"access_token": "tok", "instance_url": "https://x"})
    assert auth.mode == AuthMode.BEARER
    assert auth.access_token == "tok"
    assert auth.instance_url == "https://x"


def test_from_mapping_jwt() -> None:
    auth = SalesforceAuth.from_mapping(_jwt_credentials("https://login.salesforce.com"))
    assert auth.mode == AuthMode.JWT
    assert auth.client_id == "3MVG9-test-consumer-key"
    assert auth.username == "owner@example.com"
    assert auth.login_url == "https://login.salesforce.com"


@pytest.mark.asyncio
async def test_fetch_organization_id_jwt_needs_only_org_claim(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from types import SimpleNamespace

    from salesforce_connector import client as client_module

    seen: list[str] = []

    def post(url: str, **kwargs: object) -> SimpleNamespace:
        seen.append(url)
        return SimpleNamespace(
            status_code=200,
            json=lambda: {
                "access_token": "jwt-access-token",
                "instance_url": "https://acme.my.salesforce.com",
                "issued_at": 1_700_000_000_000,
            },
        )

    def get(url: str, **kwargs: object) -> SimpleNamespace:
        seen.append(url)
        return SimpleNamespace(status_code=200, json=lambda: {"organization_id": "00D000000000001"})

    monkeypatch.setattr(client_module.requests, "post", post)
    monkeypatch.setattr(client_module.requests, "get", get)
    auth = SalesforceAuth.from_mapping(
        {
            "client_id": "consumer-key",
            "private_key": _private_key,
            "username": "admin@example.com",
            "login_url": "https://login.salesforce.com",
        }
    )

    assert await fetch_organization_id(auth) == "00D000000000001"
    assert seen == [
        "https://login.salesforce.com/services/oauth2/token",
        "https://login.salesforce.com/services/oauth2/userinfo",
    ]


@pytest.mark.asyncio
async def test_fetch_organization_id_accepts_early_bearer_payload_without_instance_url(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from types import SimpleNamespace

    from salesforce_connector import client as client_module

    requested: list[str] = []

    def get(url: str, **kwargs: object) -> SimpleNamespace:
        requested.append(url)
        return SimpleNamespace(
            status_code=200,
            json=lambda: {"organization_id": "00D000000000001"},
        )

    monkeypatch.setattr(client_module.requests, "get", get)
    auth = SalesforceAuth.from_mapping(
        {"access_token": "setup-token", "login_url": "https://login.salesforce.com"}
    )

    assert await fetch_organization_id(auth) == "00D000000000001"
    assert requested == ["https://login.salesforce.com/services/oauth2/userinfo"]


def test_from_mapping_prefers_user_token_over_merged_org_jwt() -> None:
    auth = SalesforceAuth.from_mapping(
        {
            **_jwt_credentials("https://login.salesforce.com"),
            "access_token": "user-oauth-token",
            "instance_url": "https://example.my.salesforce.com",
        }
    )
    assert auth.mode == AuthMode.BEARER
    assert auth.access_token == "user-oauth-token"


@pytest.mark.parametrize(
    "value",
    [
        "https://evilforce.com",
        "http://login.salesforce.com",
        "https://login.salesforce.com:8443",
        "https://login.salesforce.com.evil.example",
    ],
)
def test_salesforce_login_url_rejects_untrusted_origins(value: str) -> None:
    with pytest.raises(ValueError):
        SalesforceConnector._salesforce_login_url(value)


def test_salesforce_login_url_accepts_salesforce_domains() -> None:
    assert (
        SalesforceConnector._salesforce_login_url("https://login.salesforce.com/")
        == "https://login.salesforce.com"
    )
    assert (
        SalesforceConnector._salesforce_login_url("https://acme.my.salesforce.com")
        == "https://acme.my.salesforce.com"
    )


def test_from_mapping_missing_credentials() -> None:
    with pytest.raises(ValueError):
        SalesforceAuth.from_mapping({})


def test_from_mapping_partial_jwt_falls_back_to_bearer_error() -> None:
    # username alone is not a valid JWT config and not a bearer token
    with pytest.raises(ValueError):
        SalesforceAuth.from_mapping({"username": "owner@example.com"})


# --- JWT token acquisition --------------------------------------------------------


async def test_jwt_mints_token_and_queries(
    mock_salesforce_api: MockSalesforceAPI, mock_salesforce_server: str
) -> None:
    mock_salesforce_api.reset()
    mock_salesforce_api.add_account()
    mock_salesforce_api.add_account("001000000000002", name="Beta Corp")

    client = SalesforceClient(SalesforceAuth.from_mapping(_jwt_credentials(mock_salesforce_server)))
    result = await client.query("SELECT Id, Name FROM Account ORDER BY Id")

    assert result.total_size == 2
    # one token issuance for the whole session
    assert mock_salesforce_api.token_issuances == 1
    assert client.instance_url == mock_salesforce_server


async def test_jwt_assertion_is_rs256_signed(
    mock_salesforce_api: MockSalesforceAPI, mock_salesforce_server: str
) -> None:
    mock_salesforce_api.reset()
    mock_salesforce_api.add_account()

    client = SalesforceClient(SalesforceAuth.from_mapping(_jwt_credentials(mock_salesforce_server)))
    await client.query("SELECT Id FROM Account LIMIT 1")

    header = jwt.get_unverified_header(mock_salesforce_api.last_assertion)
    claims = jwt.decode(
        mock_salesforce_api.last_assertion,
        algorithms=["RS256"],
        options={"verify_signature": False},
    )
    assert header["alg"] == "RS256"
    assert claims["iss"] == "3MVG9-test-consumer-key"
    assert claims["sub"] == "owner@example.com"
    assert claims["aud"] == mock_salesforce_server
    assert claims["exp"] - claims["iat"] == 300


async def test_jwt_refreshes_on_401(
    mock_salesforce_api: MockSalesforceAPI, mock_salesforce_server: str
) -> None:
    mock_salesforce_api.reset()
    mock_salesforce_api.add_account()
    # First API call after the initial token will 401 once, then succeed.
    mock_salesforce_api.fail_next_api_call = True

    client = SalesforceClient(SalesforceAuth.from_mapping(_jwt_credentials(mock_salesforce_server)))
    result = await client.query("SELECT Id FROM Account LIMIT 1")

    assert result.total_size == 1
    # one token for the session + one re-minted after the 401
    assert mock_salesforce_api.token_issuances == 2


async def test_bearer_mode_does_not_refresh_after_401(
    mock_salesforce_api: MockSalesforceAPI, mock_salesforce_server: str
) -> None:
    mock_salesforce_api.reset()
    mock_salesforce_api.add_account()
    mock_salesforce_api.fail_next_api_call = True

    client = SalesforceClient(
        SalesforceAuth.from_mapping(
            {"access_token": "test-token", "instance_url": mock_salesforce_server}
        )
    )
    with pytest.raises(AuthenticationError):
        await client.query("SELECT Id FROM Account LIMIT 1")
