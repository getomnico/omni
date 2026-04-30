"""Shared types for the LLM provider layer."""

from enum import StrEnum


class ProviderType(StrEnum):
    """Canonical identifier for each supported LLM provider.

    The string value is what crosses the wire (DB column, JSON API, SSE error
    payload), so changing a value is a breaking change.
    """

    ANTHROPIC = "anthropic"
    OPENAI = "openai"
    BEDROCK = "bedrock"
    OPENAI_COMPATIBLE = "openai_compatible"
    GEMINI = "gemini"
    AZURE_FOUNDRY = "azure_foundry"
    VERTEX_AI = "vertex_ai"


class ProviderError(Exception):
    """Raised when an LLM provider call fails.

    Carries enough structure for the chat router and the test-connection
    endpoint to render an actionable message in the UI without losing the
    original SDK message body.
    """

    def __init__(
        self,
        message: str,
        *,
        provider_type: ProviderType,
        model: str | None = None,
        status_code: int | None = None,
        cause: BaseException | None = None,
    ):
        super().__init__(message)
        self.message = message
        self.provider_type = provider_type
        self.model = model
        self.status_code = status_code
        if cause is not None:
            self.__cause__ = cause
