"""Microsoft Entra ID (Azure AD) authentication via client credentials flow."""

import logging
from typing import Any

from azure.identity import ClientSecretCredential

logger = logging.getLogger(__name__)

GRAPH_SCOPE = "https://graph.microsoft.com/.default"


class MSGraphAuth:
    """Handles app-only authentication for Microsoft Graph API.

    Uses client credentials flow (OAuth 2.0) with a registered app in
    Microsoft Entra ID. Requires admin-consented application permissions.
    """

    def __init__(self, tenant_id: str, client_id: str, client_secret: str):
        self._credential = ClientSecretCredential(
            tenant_id=tenant_id,
            client_id=client_id,
            client_secret=client_secret,
        )
        self._static_token: str | None = None

    @classmethod
    def from_credentials(cls, credentials: dict[str, Any]) -> "MSGraphAuth":
        if "token" in credentials:
            auth = object.__new__(cls)
            auth._credential = None
            auth._static_token = credentials["token"]
            return auth

        tenant_id = credentials.get("tenant_id")
        client_id = credentials.get("client_id")
        client_secret = credentials.get("client_secret")

        if not all([tenant_id, client_id, client_secret]):
            raise ValueError(
                "Missing required credentials: tenant_id, client_id, client_secret"
            )

        return cls(
            tenant_id=tenant_id,
            client_id=client_id,
            client_secret=client_secret,
        )

    def get_token(self) -> str:
        """Return a valid access token, refreshing if needed.

        azure-identity handles caching and refresh internally.
        """
        if self._static_token:
            return self._static_token
        token = self._credential.get_token(GRAPH_SCOPE)
        return token.token
