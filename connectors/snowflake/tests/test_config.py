import pytest
from pydantic import ValidationError

from snowflake_connector.client import _account_identifier, validate_mcp_endpoint
from snowflake_connector.config import SnowflakeConfig


def config(**overrides: object) -> SnowflakeConfig:
    values: dict[str, object] = {
        "account_url": "https://acme.snowflakecomputing.com",
        "warehouse": "OMNI_WH",
        "role": "OMNI_METADATA",
        "databases": ["ANALYTICS"],
    }
    values.update(overrides)
    return SnowflakeConfig.model_validate(values)


def test_account_identifier_is_accepted_by_the_official_connector() -> None:
    assert _account_identifier("https://acme.eu-west-1.snowflakecomputing.com") == "acme.eu-west-1"


def test_managed_endpoint_is_account_bound() -> None:
    endpoint = "https://acme.snowflakecomputing.com/api/v2/databases/ANALYTICS/schemas/PUBLIC/mcp-servers/omni"
    assert validate_mcp_endpoint(endpoint, "https://acme.snowflakecomputing.com") == endpoint


@pytest.mark.parametrize(
    "endpoint",
    [
        "http://acme.snowflakecomputing.com/api/v2/databases/A/schemas/B/mcp-servers/C",
        "https://other.snowflakecomputing.com/api/v2/databases/A/schemas/B/mcp-servers/C",
        "https://acme.snowflakecomputing.com/api/v2/databases/A/schemas/B/mcp-servers/C?token=secret",
        "https://acme.snowflakecomputing.com/not-mcp",
    ],
)
def test_managed_endpoint_rejects_unsafe_or_wrong_paths(endpoint: str) -> None:
    with pytest.raises(ValueError):
        validate_mcp_endpoint(endpoint, "https://acme.snowflakecomputing.com")


def test_source_binding_is_accepted_after_oauth_validation() -> None:
    parsed = config(
        sync_enabled=False,
        source_binding={"account": "ACME", "user": "ALICE", "email": "alice@example.com"},
    )
    assert parsed.source_binding is not None
    assert parsed.source_binding.account == "ACME"


def test_mcp_only_configuration_does_not_require_metadata_credentials() -> None:
    parsed = config(
        sync_enabled=False,
        mcp_enabled=True,
        mcp_endpoint_url="https://acme.snowflakecomputing.com/api/v2/databases/D/schemas/S/mcp-servers/M",
    )
    parsed.validate_for_use()


def test_metadata_sync_requires_metadata_configuration() -> None:
    with pytest.raises(ValueError, match="metadata sync"):
        config(warehouse=None, role=None, databases=[]).validate_for_use()


def test_mcp_requires_endpoint_when_used() -> None:
    with pytest.raises(ValueError):
        config(mcp_enabled=True).validate_for_use()


def test_unknown_configuration_is_rejected() -> None:
    with pytest.raises(ValidationError):
        config(untrusted_secret="must-not-be-accepted")
