"""Unit tests: JWT bearer auth — credential parsing, token acquisition, self-refresh."""

from __future__ import annotations

import os

import jwt
import pytest
from cryptography.hazmat.primitives import serialization
from cryptography.hazmat.primitives.asymmetric import rsa

from salesforce_connector.client import AuthenticationError, SalesforceClient
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
def test_mcp_login_url_rejects_untrusted_origins(value: str) -> None:
    with pytest.raises(ValueError):
        SalesforceConnector._mcp_login_url(value)


def test_mcp_login_url_accepts_salesforce_domains() -> None:
    assert (
        SalesforceConnector._mcp_login_url("https://login.salesforce.com/")
        == "https://login.salesforce.com"
    )
    assert (
        SalesforceConnector._mcp_login_url("https://acme.my.salesforce.com")
        == "https://acme.my.salesforce.com"
    )


def test_mcp_env_isolates_source_and_user() -> None:
    env = SalesforceConnector().prepare_mcp_env(
        {
            "source_id": "source-1",
            "user_id": "user-1",
            "credentials": {
                "access_token": "user-oauth-token",
                "instance_url": "https://acme.my.salesforce.com",
                "organization_id": "00D000000000001",
                "login_url": "https://test.salesforce.com",
            },
        }
    )
    assert env["OMNI_SALESFORCE_SOURCE_ID"] == "source-1:user-1"
    assert env["SF_ACCESS_TOKEN"] == "user-oauth-token"
    assert env["SF_ORG_ID"] == "00D000000000001"
    assert env["SF_LOGIN_URL"] == "https://test.salesforce.com"


@pytest.fixture
def mcp_workspace(monkeypatch: pytest.MonkeyPatch, tmp_path) -> str:
    workspace = tmp_path / "workspace"
    workspace.mkdir()
    monkeypatch.setenv("OMNI_SALESFORCE_MCP_WORKSPACE", str(workspace))
    return str(workspace)


def test_mcp_tool_directory_confines_unmounted_paths_to_workspace(
    mcp_workspace: str,
) -> None:
    connector = SalesforceConnector()
    arguments = connector.prepare_mcp_tool_arguments(
        "run_soql_query", {"directory": "/scratch/not-mounted", "query": "SELECT Id"}
    )

    assert arguments["directory"] == os.path.realpath(mcp_workspace)
    assert arguments["query"] == "SELECT Id"


def test_mcp_tool_directory_preserves_workspace_subdirectories(
    mcp_workspace: str,
) -> None:
    connector = SalesforceConnector()
    arguments = connector.prepare_mcp_tool_arguments(
        "run_soql_query", {"directory": "exports/today"}
    )
    workspace = os.path.realpath(mcp_workspace)
    assert arguments["directory"] == os.path.join(workspace, "exports", "today")


def test_mcp_tool_directory_rejects_traversal(mcp_workspace: str) -> None:
    connector = SalesforceConnector()
    arguments = connector.prepare_mcp_tool_arguments(
        "run_soql_query", {"directory": "../../etc/passwd"}
    )
    assert arguments["directory"] == os.path.realpath(mcp_workspace)


def test_mcp_tool_directory_rejects_symlink_escape(mcp_workspace: str, tmp_path) -> None:
    workspace = os.path.realpath(mcp_workspace)
    outside = tmp_path / "outside"
    outside.mkdir()
    link = os.path.join(workspace, "escape")
    os.symlink(str(outside), link)
    arguments = SalesforceConnector().prepare_mcp_tool_arguments(
        "run_soql_query", {"directory": "escape"}
    )
    assert arguments["directory"] == workspace
    assert arguments["directory"] != str(outside)


def test_mcp_action_classification_uses_explicit_allowlist() -> None:
    from omni_connector import ActionDefinition

    connector = SalesforceConnector()

    def classify(name: str) -> str:
        return connector._classify_mcp_action(
            ActionDefinition(name=name, description="", mode="write", origin="mcp")
        ).mode

    # Known read-only Salesforce MCP data/core tools.
    assert classify("run_soql_query") == "read"
    assert classify("get_username") == "read"
    # Known mutating tools stay writes even though their names look readable.
    assert classify("assign_permission_set") == "write"
    assert classify("run_apex_test") == "write"
    # Unknown tools default to write.
    assert classify("do_thing") == "write"
    assert classify("query_something_else") == "write"
    # Native actions are never reclassified.
    native = connector._classify_mcp_action(
        ActionDefinition(name="create_case", description="", mode="read", origin="native")
    )
    assert native.mode == "read"


def test_mcp_tool_directory_rejects_malformed_values() -> None:
    with pytest.raises(ValueError):
        SalesforceConnector().prepare_mcp_tool_arguments("run_soql_query", {"directory": 42})


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
