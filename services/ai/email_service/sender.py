"""Email sending with pluggable providers."""

import logging
from abc import ABC, abstractmethod
from dataclasses import dataclass
from email.mime.multipart import MIMEMultipart
from email.mime.text import MIMEText
from typing import Optional

import aiosmtplib
import httpx
from asyncpg import Pool
from azure.communication.email import EmailClient

from .repository import (
    ACSConfig,
    EmailProviderConfig,
    ResendConfig,
    SMTPConfig,
    get_current_email_provider,
)

logger = logging.getLogger(__name__)


@dataclass
class SendResult:
    success: bool
    message_id: Optional[str] = None
    error: Optional[str] = None


class EmailProvider(ABC):
    @abstractmethod
    async def send(
        self, to: str, subject: str, html: str, text: Optional[str] = None
    ) -> SendResult: ...


class ACSEmailProvider(EmailProvider):
    def __init__(self, config: ACSConfig):
        self._client = EmailClient.from_connection_string(config.connection_string)
        self._sender_address = config.sender_address

    async def send(
        self, to: str, subject: str, html: str, text: Optional[str] = None
    ) -> SendResult:
        try:
            message = {
                "senderAddress": self._sender_address,
                "content": {"subject": subject, "html": html, "plainText": text or ""},
                "recipients": {"to": [{"address": to}]},
            }

            poller = self._client.begin_send(message)
            result = poller.result()

            if result["status"] == "Succeeded":
                return SendResult(success=True, message_id=result.get("id"))

            logger.error("ACS send failed: status=%s", result["status"])
            return SendResult(success=False, error=f"Send failed: {result['status']}")
        except Exception as e:
            logger.error("ACS send error: %s", e)
            return SendResult(success=False, error="Failed to send email via ACS")


class ResendEmailProvider(EmailProvider):
    def __init__(self, config: ResendConfig):
        self._api_key = config.api_key
        self._from_email = config.from_email

    async def send(
        self, to: str, subject: str, html: str, text: Optional[str] = None
    ) -> SendResult:
        try:
            async with httpx.AsyncClient() as client:
                resp = await client.post(
                    "https://api.resend.com/emails",
                    headers={
                        "Authorization": f"Bearer {self._api_key}",
                        "Content-Type": "application/json",
                    },
                    json={
                        "from": self._from_email,
                        "to": [to],
                        "subject": subject,
                        "html": html,
                    },
                    timeout=30,
                )

            if resp.status_code == 200:
                data = resp.json()
                return SendResult(success=True, message_id=data.get("id"))

            logger.error("Resend error: status=%d body=%s", resp.status_code, resp.text)
            return SendResult(success=False, error="Failed to send email via Resend")
        except Exception as e:
            logger.error("Resend send error: %s", e)
            return SendResult(success=False, error="Failed to send email via Resend")


class SMTPEmailProvider(EmailProvider):
    def __init__(self, config: SMTPConfig):
        self._host = config.host
        self._port = config.port
        self._user = config.user
        self._password = config.password
        self._secure = config.secure
        self._from_email = config.from_email

    async def send(
        self, to: str, subject: str, html: str, text: Optional[str] = None
    ) -> SendResult:
        try:
            msg = MIMEMultipart("alternative")
            msg["Subject"] = subject
            msg["From"] = self._from_email
            msg["To"] = to

            if text:
                msg.attach(MIMEText(text, "plain"))
            msg.attach(MIMEText(html, "html"))

            await aiosmtplib.send(
                msg,
                hostname=self._host,
                port=self._port,
                username=self._user,
                password=self._password,
                use_tls=self._secure,
                start_tls=not self._secure,
            )

            return SendResult(success=True)
        except Exception as e:
            logger.error("SMTP send error: %s", e)
            return SendResult(success=False, error="Failed to send email via SMTP")


def _build_provider(config: EmailProviderConfig) -> EmailProvider:
    match config:
        case ACSConfig():
            return ACSEmailProvider(config)
        case ResendConfig():
            return ResendEmailProvider(config)
        case SMTPConfig():
            return SMTPEmailProvider(config)


class EmailSender:
    """Reads provider config from DB and delegates to the appropriate EmailProvider."""

    def __init__(self, pool: Optional[Pool] = None):
        self._pool = pool
        self._provider: Optional[EmailProvider] = None

    async def send(
        self, to: str, subject: str, html: str, text: Optional[str] = None
    ) -> SendResult:
        if not self._provider:
            config = await get_current_email_provider(self._pool)
            if not config:
                return SendResult(success=False, error="No email provider configured")
            self._provider = _build_provider(config)

        return await self._provider.send(to, subject, html, text)

    def reset(self):
        self._provider = None
