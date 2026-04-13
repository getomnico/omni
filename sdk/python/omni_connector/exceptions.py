class ConnectorError(Exception):
    """Base exception for connector errors."""

    pass


class SdkClientError(ConnectorError):
    """Error communicating with connector-manager SDK endpoints."""

    pass


class SyncCancelledError(ConnectorError):
    """Raised when a sync is cancelled."""

    pass


class ServiceOverloadedError(SdkClientError):
    """Raised when the extraction service is overloaded (HTTP 429)."""

    def __init__(self, message: str, retry_after: int = 30):
        super().__init__(message)
        self.retry_after = retry_after


class ConfigurationError(ConnectorError):
    """Error in connector configuration."""

    pass
