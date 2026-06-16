"""Configuration parsing for the Google Ads connector."""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any, cast

DEFAULT_ENTITY_TYPES = [
    "customer",
    "campaign_budget",
    "bidding_strategy",
    "campaign",
    "ad_group",
    "ad_group_ad",
    "asset",
    "keyword_view",
    "shared_set",
    "user_list",
    "conversion_action",
]

ENTITY_TYPES_WITH_CHANGE_STATUS = {
    "campaign",
    "campaign_budget",
    "bidding_strategy",
    "ad_group",
    "ad_group_ad",
    "asset",
    "keyword_view",
    "shared_set",
    "user_list",
    "conversion_action",
}

GOOGLE_ADS_SCOPE = "https://www.googleapis.com/auth/adwords"


def normalize_customer_id(value: str | int | None) -> str | None:
    if value is None:
        return None
    normalized = str(value).replace("-", "").strip()
    return normalized or None


def _string_list(value: Any) -> list[str]:
    if value is None:
        return []
    if isinstance(value, str):
        return [
            v
            for v in (normalize_customer_id(p) for p in value.replace(",", " ").split())
            if v
        ]
    if isinstance(value, list):
        return [v for v in (normalize_customer_id(p) for p in value) if v]
    return []


@dataclass(frozen=True)
class GoogleAdsSourceConfig:
    customer_ids: list[str]
    login_customer_id: str | None = None
    entity_types: list[str] = field(default_factory=lambda: list(DEFAULT_ENTITY_TYPES))
    sync_enabled: bool = True
    api_version: str | None = None

    @classmethod
    def parse(
        cls, source_config: dict[str, Any], credentials: dict[str, Any]
    ) -> GoogleAdsSourceConfig:
        customer_ids = _string_list(
            source_config.get("customer_ids")
            or source_config.get("selected_customer_ids")
            or credentials.get("customer_ids")
            or credentials.get("selected_customer_ids")
        )
        if not customer_ids:
            single = normalize_customer_id(
                source_config.get("customer_id") or credentials.get("customer_id")
            )
            if single:
                customer_ids = [single]
        if not customer_ids:
            raise ValueError("At least one Google Ads customer ID is required")

        login_customer_id = normalize_customer_id(
            source_config.get("login_customer_id")
            or credentials.get("login_customer_id")
        )
        raw_entity_types = source_config.get("entity_types") or source_config.get(
            "enabled_entities"
        )
        entity_types = (
            [str(e) for e in raw_entity_types]
            if isinstance(raw_entity_types, list)
            else []
        )
        if not entity_types:
            entity_types = list(DEFAULT_ENTITY_TYPES)
        entity_types = [e for e in entity_types if e in DEFAULT_ENTITY_TYPES]
        if not entity_types:
            entity_types = list(DEFAULT_ENTITY_TYPES)

        return cls(
            customer_ids=customer_ids,
            login_customer_id=login_customer_id,
            entity_types=entity_types,
            sync_enabled=bool(source_config.get("sync_enabled", True)),
            api_version=(
                source_config.get("api_version")
                if isinstance(source_config.get("api_version"), str)
                else None
            ),
        )


@dataclass(frozen=True)
class GoogleAdsCredentials:
    developer_token: str
    access_token: str | None = None
    refresh_token: str | None = None
    client_id: str | None = None
    client_secret: str | None = None
    token_uri: str = "https://oauth2.googleapis.com/token"
    login_customer_id: str | None = None
    use_proto_plus: bool = True

    @classmethod
    def parse(cls, raw: dict[str, Any]) -> GoogleAdsCredentials:
        raw_credentials = raw.get("credentials")
        raw_config = raw.get("config")
        envelope_credentials = cast(
            dict[str, Any], raw_credentials if isinstance(raw_credentials, dict) else {}
        )
        envelope_config = cast(
            dict[str, Any], raw_config if isinstance(raw_config, dict) else {}
        )
        payload: dict[str, Any] = {**raw, **envelope_config, **envelope_credentials}

        developer_token = payload.get("developer_token") or payload.get(
            "google_ads_developer_token"
        )
        if not developer_token:
            raise ValueError("Missing developer_token for Google Ads")

        access_token = payload.get("access_token") or payload.get("token")
        refresh_token = payload.get("refresh_token")
        client_id = payload.get("client_id") or payload.get("oauth_client_id")
        client_secret = payload.get("client_secret") or payload.get(
            "oauth_client_secret"
        )
        login_customer_id = normalize_customer_id(payload.get("login_customer_id"))

        return cls(
            developer_token=str(developer_token),
            access_token=str(access_token) if access_token else None,
            refresh_token=str(refresh_token) if refresh_token else None,
            client_id=str(client_id) if client_id else None,
            client_secret=str(client_secret) if client_secret else None,
            token_uri=str(
                payload.get("token_uri") or "https://oauth2.googleapis.com/token"
            ),
            login_customer_id=login_customer_id,
        )

    def to_google_ads_dict(
        self, login_customer_id: str | None = None
    ) -> dict[str, Any]:
        cfg: dict[str, Any] = {
            "developer_token": self.developer_token,
            "use_proto_plus": self.use_proto_plus,
        }
        effective_login = login_customer_id or self.login_customer_id
        if effective_login:
            cfg["login_customer_id"] = effective_login
        if self.client_id:
            cfg["client_id"] = self.client_id
        if self.client_secret:
            cfg["client_secret"] = self.client_secret
        if self.refresh_token:
            cfg["refresh_token"] = self.refresh_token
        if self.token_uri:
            cfg["token_uri"] = self.token_uri
        return cfg
