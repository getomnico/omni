from __future__ import annotations

from urllib.parse import urlparse

from pydantic import BaseModel, ConfigDict, Field, field_validator


class SnowflakeConfig(BaseModel):
    model_config = ConfigDict(extra="forbid")

    account_url: str
    warehouse: str
    role: str
    databases: list[str] = Field(min_length=1)
    schemas_allowlist: list[str] | None = None
    schemas_denylist: list[str] | None = None
    included_object_types: list[str] = Field(
        default_factory=lambda: [
            "TABLE",
            "VIEW",
            "EXTERNAL TABLE",
            "EVENT TABLE",
            "MATERIALIZED VIEW",
        ]
    )
    sync_enabled: bool = True
    mcp_endpoint_url: str | None = None
    mcp_enabled: bool = False
    write_tools_enabled: bool = False
    include_tags: bool = False
    read_only: bool = True

    @field_validator("account_url", "mcp_endpoint_url")
    @classmethod
    def validate_url(cls, value: str | None, info: object) -> str | None:
        if value is None:
            return value
        parsed = urlparse(value)
        if (
            parsed.scheme != "https"
            or parsed.username
            or parsed.password
            or parsed.query
            or parsed.fragment
        ):
            raise ValueError("Snowflake endpoints must be credential-free HTTPS URLs")
        if parsed.port not in (None, 443):
            raise ValueError("Snowflake endpoints may only use the default HTTPS port")
        if not parsed.hostname:
            raise ValueError("Snowflake endpoint must include a hostname")
        return value.rstrip("/")

    @field_validator("mcp_enabled")
    @classmethod
    def mcp_requires_endpoint(cls, value: bool, info: object) -> bool:
        # Cross-field validation is performed by validate_for_use so Pydantic
        # can still parse partially completed settings forms.
        return value

    def validate_for_use(self) -> None:
        if self.mcp_enabled and self.mcp_endpoint_url is None:
            raise ValueError("mcp_endpoint_url is required when MCP is enabled")
        if self.schemas_allowlist is not None and self.schemas_denylist is not None:
            overlap = set(self.schemas_allowlist) & set(self.schemas_denylist)
            if overlap:
                raise ValueError("schema allowlist and denylist overlap")


class SnowflakeCredentials(BaseModel):
    model_config = ConfigDict(extra="forbid")

    username: str
    private_key: str
    private_key_passphrase: str | None = None
    mcp_oauth_client_id: str | None = None
    mcp_oauth_client_secret: str | None = None
    account_url: str | None = None
