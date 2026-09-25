import pytest
from omni_connector import OAuthCredentialFlow, Source

from snowflake_connector.connector import SnowflakeConnector


class IdentitySession:
    def __init__(
        self, account: str = "ACME", user: str = "ALICE", email: str = "alice@example.com"
    ) -> None:
        self.account = account
        self.user = user
        self.email = email

    def execute(self, statement: str) -> list[dict[str, str]]:
        if statement.startswith("SELECT CURRENT_ACCOUNT"):
            return [{"ACCOUNT": self.account, "USER": self.user, "REGION": "AWS_US_EAST_1"}]
        return [{"PROPERTY": "EMAIL", "PROPERTY_VALUE": self.email}]

    def close(self) -> None:
        pass


def source() -> Source:
    return Source.model_validate(
        {
            "id": "src",
            "name": "Snowflake",
            "source_type": "snowflake",
            "config": {
                "account_url": "https://acme.snowflakecomputing.com",
                "warehouse": "W",
                "role": "R",
                "databases": ["D"],
                "mcp_enabled": True,
                "mcp_endpoint_url": "https://acme.snowflakecomputing.com/api/v2/databases/D/schemas/S/mcp-servers/M",
                "source_binding": {"account": "ACME", "user": "BOUND"},
            },
            "is_active": True,
            "is_deleted": False,
            "scope": "org",
            "created_at": "2026-06-23T10:00:00Z",
            "updated_at": "2026-06-23T10:00:00Z",
            "created_by": "admin",
        }
    )


@pytest.mark.asyncio
async def test_oauth_validation_binds_provider_principal() -> None:
    connector = SnowflakeConnector(oauth_session_factory=lambda _config, _token: IdentitySession())
    binding = await connector.validate_oauth_credential(
        source(),
        {"access_token": "oauth-token"},
        OAuthCredentialFlow.USER_READ,
        {"omni_user_email": "ALICE@example.com"},
    )
    assert binding == {"account": "ACME", "user": "ALICE", "email": "alice@example.com"}


@pytest.mark.asyncio
async def test_oauth_validation_rejects_wrong_principal() -> None:
    connector = SnowflakeConnector(oauth_session_factory=lambda _config, _token: IdentitySession())
    with pytest.raises(ValueError, match="does not match"):
        await connector.validate_oauth_credential(
            source(),
            {"access_token": "oauth-token"},
            OAuthCredentialFlow.USER_READ,
            {"omni_user_email": "mallory@example.com"},
        )
