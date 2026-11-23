from __future__ import annotations

import pytest
from anthropic import AsyncAnthropicFoundry
from openai import AsyncOpenAI

from providers import ProviderType, create_llm_provider
from providers import azure_foundry
from providers.azure_foundry import AzureFoundryProvider

pytestmark = pytest.mark.unit

ENDPOINT = "https://example-resource.services.ai.azure.com"
API_KEY = "test-azure-key"


@pytest.fixture
def forbid_managed_identity(monkeypatch: pytest.MonkeyPatch) -> None:
    def fail() -> None:
        raise AssertionError("DefaultAzureCredential must not be used with an API key")

    monkeypatch.setattr(azure_foundry, "DefaultAzureCredential", fail)


@pytest.mark.usefixtures("forbid_managed_identity")
@pytest.mark.parametrize(
    ("model", "client_type", "base_path"),
    [
        ("claude-sonnet-5-5", AsyncAnthropicFoundry, "anthropic/"),
        ("gpt-6.1-sol", AsyncOpenAI, "openai/v1/"),
    ],
)
def test_api_key_authenticates_each_model_family(
    model: str, client_type: type, base_path: str
) -> None:
    provider = AzureFoundryProvider(ENDPOINT, model, api_key=API_KEY)

    client = provider._delegate.client
    assert isinstance(client, client_type)
    assert client.api_key == API_KEY
    assert str(client.base_url) == f"{ENDPOINT}/{base_path}"


@pytest.mark.parametrize("model", ["claude-opus-5-5", "gpt-5.6-terra"])
def test_without_api_key_falls_back_to_managed_identity(
    monkeypatch: pytest.MonkeyPatch, model: str
) -> None:
    credentials: list[object] = []

    def fake_credential() -> object:
        credential = object()
        credentials.append(credential)
        return credential

    monkeypatch.setattr(azure_foundry, "DefaultAzureCredential", fake_credential)
    monkeypatch.setattr(
        azure_foundry,
        "get_bearer_token_provider",
        lambda credential, scope: lambda: "token",
    )

    AzureFoundryProvider(ENDPOINT, model)

    assert len(credentials) == 1


@pytest.mark.usefixtures("forbid_managed_identity")
def test_create_llm_provider_forwards_api_key() -> None:
    provider = create_llm_provider(
        ProviderType.AZURE_FOUNDRY,
        endpoint_url=ENDPOINT,
        model="claude-haiku-4-5",
        api_key=API_KEY,
    )

    assert isinstance(provider, AzureFoundryProvider)
    assert provider._delegate.client.api_key == API_KEY
