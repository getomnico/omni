"""Microsoft Entra ID (Azure AD) authentication for org-wide sync and per-user OAuth."""

from __future__ import annotations

import logging
from typing import Any

from azure.identity import ClientSecretCredential
from pydantic import BaseModel, ConfigDict, TypeAdapter

logger = logging.getLogger(__name__)

GRAPH_SCOPE = "https://graph.microsoft.com/.default"


class MSClientSecretCreds(BaseModel):
    """Org-wide app-only credentials (client credentials flow)."""

    model_config = ConfigDict(extra="forbid")

    tenant_id: str
    client_id: str
    client_secret: str


class MSUserOAuthCreds(BaseModel):
    """Per-user delegated bearer token issued by the web OAuth callback."""

    model_config = ConfigDict(extra="forbid")

    access_token: str
    refresh_token: str | None = None
    token_type: str | None = None


class MSStaticTokenCreds(BaseModel):
    """Pre-fetched bearer token, used by tests and managed-identity flows."""

    model_config = ConfigDict(extra="forbid")

    token: str


MSCredentials = MSClientSecretCreds | MSUserOAuthCreds | MSStaticTokenCreds
_ms_credentials_adapter: TypeAdapter[MSCredentials] = TypeAdapter(MSCredentials)


def parse_ms_credentials(raw: dict[str, Any]) -> MSCredentials:
    return _ms_credentials_adapter.validate_python(raw)


class MSGraphAuth:
    """Handles authentication for Microsoft Graph API.

    Supports app-only client credentials (org-wide sync) and per-user
    delegated OAuth bearer tokens (tool calls).
    """

    def __init__(self, tenant_id: str, client_id: str, client_secret: str):
        self._credential = ClientSecretCredential(
            tenant_id=tenant_id,
            client_id=client_id,
            client_secret=client_secret,
        )
        # Only used in testing
        self._static_token: str | None = None

    @classmethod
    def from_credentials(cls, creds: MSCredentials) -> MSGraphAuth:
        match creds:
            case MSClientSecretCreds():
                return cls(
                    tenant_id=creds.tenant_id,
                    client_id=creds.client_id,
                    client_secret=creds.client_secret,
                )
            case MSUserOAuthCreds():
                return cls._from_static_token(creds.access_token)
            case MSStaticTokenCreds():
                return cls._from_static_token(creds.token)

    @classmethod
    def _from_static_token(cls, token: str) -> MSGraphAuth:
        auth = object.__new__(cls)
        auth._credential = None
        auth._static_token = token
        return auth

    def get_token(self) -> str:
        """Return a valid access token, refreshing if needed.

        azure-identity handles caching and refresh internally.
        """
        if self._static_token:
            return self._static_token
        token = self._credential.get_token(GRAPH_SCOPE)
        return token.token
