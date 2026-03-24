"""Read-only DB access for email provider configuration."""

import json
import logging
from dataclasses import dataclass
from typing import Literal, Optional, Union

from asyncpg import Pool

from crypto import decrypt_config
from db.connection import get_db_pool

logger = logging.getLogger(__name__)


@dataclass
class ACSConfig:
    type: Literal["acs"]
    connection_string: str
    sender_address: str


@dataclass
class ResendConfig:
    type: Literal["resend"]
    api_key: str
    from_email: str


@dataclass
class SMTPConfig:
    type: Literal["smtp"]
    host: str
    from_email: str
    port: int = 587
    user: Optional[str] = None
    password: Optional[str] = None
    secure: bool = False


EmailProviderConfig = Union[ACSConfig, ResendConfig, SMTPConfig]


def _parse_config(provider_type: str, config: dict) -> EmailProviderConfig:
    if provider_type == "acs":
        return ACSConfig(
            type="acs",
            connection_string=config["connectionString"],
            sender_address=config["senderAddress"],
        )
    elif provider_type == "resend":
        return ResendConfig(
            type="resend",
            api_key=config["apiKey"],
            from_email=config["fromEmail"],
        )
    elif provider_type == "smtp":
        return SMTPConfig(
            type="smtp",
            host=config["host"],
            from_email=config["fromEmail"],
            port=config.get("port", 587),
            user=config.get("user"),
            password=config.get("password"),
            secure=config.get("secure", False),
        )
    else:
        raise ValueError(f"Unknown email provider type: {provider_type}")


async def get_current_email_provider(
    pool: Optional[Pool] = None,
) -> Optional[EmailProviderConfig]:
    if not pool:
        pool = await get_db_pool()

    row = await pool.fetchrow(
        """
        SELECT provider_type, config
        FROM email_providers
        WHERE is_current = true AND is_deleted = false
        LIMIT 1
        """
    )

    if not row:
        return None

    config = row["config"]
    if isinstance(config, str):
        config = json.loads(config)
    config = decrypt_config(config)

    return _parse_config(row["provider_type"], config)
