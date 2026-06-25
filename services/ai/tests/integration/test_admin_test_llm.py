"""API contract tests for POST /admin/provider/{provider_type}/test."""

from unittest.mock import AsyncMock, patch

import pytest
from fastapi import FastAPI
from httpx import ASGITransport, AsyncClient

from providers import ProviderError, ProviderType
from routers.model_providers import router as model_providers_router


def _build_app() -> FastAPI:
    app = FastAPI()
    app.include_router(model_providers_router)
    return app


@pytest.fixture
async def admin_client():
    async with AsyncClient(
        transport=ASGITransport(app=_build_app()), base_url="http://test"
    ) as client:
        yield client


@pytest.mark.integration
async def test_test_provider_surfaces_provider_error(admin_client):
    fake_provider = AsyncMock()
    fake_provider.generate_response = AsyncMock(
        side_effect=ProviderError(
            "404: Model use case details have not been submitted for this account.",
            provider_type=ProviderType.BEDROCK,
            model="anthropic.claude-sonnet-4-20250514-v1:0",
            status_code=404,
        )
    )
    fake_provider.model_name = "anthropic.claude-sonnet-4-20250514-v1:0"
    fake_provider.provider_type = ProviderType.BEDROCK

    with patch(
        "routers.model_providers.create_llm_provider", return_value=fake_provider
    ):
        resp = await admin_client.post(
            "/admin/provider/bedrock/test",
            json={
                "region_name": "us-east-1",
                "model_id": "anthropic.claude-sonnet-4-20250514-v1:0",
            },
        )

    assert resp.status_code == 200
    body = resp.json()
    assert body["ok"] is False
    assert "Model use case details" in body["error"]
    assert body["provider"] == "bedrock"
    assert body["status_code"] == 404
    assert body["model"] == "anthropic.claude-sonnet-4-20250514-v1:0"


@pytest.mark.integration
async def test_test_provider_uses_provider_type_from_path(admin_client):
    fake_provider = AsyncMock()
    fake_provider.generate_response = AsyncMock(return_value=("Hi there", None))
    fake_provider.model_name = "llama-3"
    fake_provider.provider_type = ProviderType.OPENAI_COMPATIBLE

    with patch(
        "routers.model_providers.create_llm_provider", return_value=fake_provider
    ) as create_provider:
        resp = await admin_client.post(
            "/admin/provider/openai_compatible/test",
            json={
                "base_url": "http://llama-cpp:8000",
                "api_key": "x",
                "model": "llama-3",
            },
        )

    assert resp.status_code == 200
    assert resp.json()["ok"] is True
    create_provider.assert_called_once_with(
        ProviderType.OPENAI_COMPATIBLE,
        base_url="http://llama-cpp:8000",
        api_key="x",
        model="llama-3",
    )


@pytest.mark.integration
async def test_openai_compatible_without_model_tests_models_endpoint(admin_client):
    list_models = AsyncMock(return_value="deepseek-v4-flash")

    with patch(
        "routers.model_providers._test_openai_compatible_connection_without_model",
        list_models,
    ):
        resp = await admin_client.post(
            "/admin/provider/openai_compatible/test",
            json={
                "base_url": "https://api.deepseek.com",
                "api_key": "x",
            },
        )

    assert resp.status_code == 200
    body = resp.json()
    assert body["ok"] is True
    assert body["model"] == "deepseek-v4-flash"
    list_models.assert_awaited_once()


@pytest.mark.integration
async def test_bedrock_without_model_tests_provider_level_connection(admin_client):
    fake_provider = AsyncMock()
    fake_provider.model_name = "default-bedrock-model"
    fake_provider.provider_type = ProviderType.BEDROCK
    test_connection = AsyncMock(return_value="anthropic.claude-sonnet-4-5")

    with (
        patch("routers.model_providers.create_llm_provider", return_value=fake_provider),
        patch(
            "routers.model_providers._test_bedrock_connection_without_model",
            test_connection,
        ),
    ):
        resp = await admin_client.post(
            "/admin/provider/bedrock/test",
            json={"region_name": "us-east-1"},
        )

    assert resp.status_code == 200
    body = resp.json()
    assert body["ok"] is True
    assert body["model"] == "anthropic.claude-sonnet-4-5"
    test_connection.assert_awaited_once()


@pytest.mark.integration
async def test_azure_foundry_without_model_tests_provider_level_connection(admin_client):
    fake_provider = AsyncMock()
    fake_provider.model_name = "gpt-4o"
    fake_provider.provider_type = ProviderType.AZURE_FOUNDRY
    test_connection = AsyncMock(return_value="gpt-4o")

    with (
        patch("routers.model_providers.create_llm_provider", return_value=fake_provider),
        patch(
            "routers.model_providers._test_azure_foundry_connection_without_model",
            test_connection,
        ),
    ):
        resp = await admin_client.post(
            "/admin/provider/azure_foundry/test",
            json={"endpoint_url": "https://example.services.ai.azure.com"},
        )

    assert resp.status_code == 200
    body = resp.json()
    assert body["ok"] is True
    assert body["model"] == "gpt-4o"
    test_connection.assert_awaited_once()


@pytest.mark.integration
async def test_vertex_ai_without_model_tests_provider_level_connection(admin_client):
    fake_provider = AsyncMock()
    fake_provider.model_name = "gemini-2.5-flash"
    fake_provider.provider_type = ProviderType.VERTEX_AI
    test_connection = AsyncMock(return_value="publishers/google/models/gemini-2.5-flash")

    with (
        patch("routers.model_providers.create_llm_provider", return_value=fake_provider),
        patch(
            "routers.model_providers._test_vertex_ai_connection_without_model",
            test_connection,
        ),
    ):
        resp = await admin_client.post(
            "/admin/provider/vertex_ai/test",
            json={"region": "us-central1", "project_id": "project"},
        )

    assert resp.status_code == 200
    body = resp.json()
    assert body["ok"] is True
    assert body["model"] == "publishers/google/models/gemini-2.5-flash"
    test_connection.assert_awaited_once()
