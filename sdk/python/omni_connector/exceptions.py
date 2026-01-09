class ConnectorError(Exception):
    """Base exception for connector errors."""

    pass


class SdkClientError(ConnectorError):
    """Error communicating with connector-manager SDK endpoints."""

    pass


class SyncCancelledError(ConnectorError):
    """Raised when a sync is cancelled."""

    pass


class ConfigurationError(ConnectorError):
    """Error in connector configuration."""

    pass
