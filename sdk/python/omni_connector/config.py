from __future__ import annotations

import os
from dataclasses import dataclass


@dataclass(frozen=True, slots=True)
class SdkConfig:
    """Runtime configuration shared by the connector server and SDK client."""

    connector_manager_url: str
    connector_host_name: str | None = None
    port: int = 8000
    request_timeout: float = 30.0

    def __post_init__(self) -> None:
        manager_url = self.connector_manager_url.strip().rstrip("/")
        if not manager_url:
            raise ValueError("CONNECTOR_MANAGER_URL must not be empty")
        if self.port < 1 or self.port > 65535:
            raise ValueError("Connector port must be between 1 and 65535")
        if self.request_timeout <= 0:
            raise ValueError("SDK request timeout must be greater than zero")
        object.__setattr__(self, "connector_manager_url", manager_url)

    @property
    def connector_url(self) -> str:
        if not self.connector_host_name:
            raise ValueError(
                "CONNECTOR_HOST_NAME environment variable is required. "
                "Set it to this connector's hostname (e.g. the Docker service name)."
            )
        return f"http://{self.connector_host_name}:{self.port}"

    @classmethod
    def from_env(cls, *, port: int | None = None) -> SdkConfig:
        manager_url = os.environ.get("CONNECTOR_MANAGER_URL")
        if not manager_url:
            raise ValueError("CONNECTOR_MANAGER_URL environment variable not set")

        configured_port = port
        if configured_port is None:
            raw_port = os.environ.get("PORT", "8000")
            try:
                configured_port = int(raw_port)
            except ValueError as exc:
                raise ValueError("PORT must be a valid integer") from exc

        raw_timeout = os.environ.get("OMNI_SDK_REQUEST_TIMEOUT_SECONDS", "30")
        try:
            request_timeout = float(raw_timeout)
        except ValueError as exc:
            raise ValueError(
                "OMNI_SDK_REQUEST_TIMEOUT_SECONDS must be a valid number"
            ) from exc

        return cls(
            connector_manager_url=manager_url,
            connector_host_name=os.environ.get("CONNECTOR_HOST_NAME"),
            port=configured_port,
            request_timeout=request_timeout,
        )
