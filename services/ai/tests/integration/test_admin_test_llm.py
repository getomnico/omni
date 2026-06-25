"""API contract tests for POST /admin/provider/{provider_type}/test."""

from unittest.mock import AsyncMock, patch

import pytest
from fastapi import FastAPI
from httpx import ASGITransport, AsyncClient

from providers import ProviderError, ProviderType
from routers.model_providers import AvailableModel
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
    fake_provider.model_name = "anthropic.claude-sonnet-4-20250514-v1:0"
    fake_provider.provider_type = ProviderType.BEDROCK
    list_models = AsyncMock(
        side_effect=ProviderError(
            "404: Model use case details have not been submitted for this account.",
            provider_type=ProviderType.BEDROCK,
            model="anthropic.claude-sonnet-4-20250514-v1:0",
            status_code=404,
        )
    )

    with (
        patch("routers.model_providers.create_llm_provider", return_value=fake_provider),
        patch("routers.model_providers._list_provider_models", list_models),
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
    assert body["model"] is None


@pytest.mark.integration
async def test_test_provider_uses_provider_type_from_path(admin_client):
    fake_provider = AsyncMock()
    fake_provider.model_name = "llama-3"
    fake_provider.provider_type = ProviderType.OPENAI_COMPATIBLE
    list_models = AsyncMock(
        return_value=[AvailableModel(model_id="llama-3", display_name="llama-3")]
    )

    with (
        patch(
            "routers.model_providers.create_llm_provider", return_value=fake_provider
        ) as create_provider,
        patch("routers.model_providers._list_provider_models", list_models),
    ):
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
    assert resp.json()["model"] == "llama-3"
    list_models.assert_awaited_once()
    create_provider.assert_called_once_with(
        ProviderType.OPENAI_COMPATIBLE,
        base_url="http://llama-cpp:8000",
        api_key="x",
        model="llama-3",
    )


@pytest.mark.integration
async def test_openai_compatible_without_model_tests_models_endpoint(admin_client):
    list_models = AsyncMock(
        return_value=[AvailableModel(model_id="deepseek-v4-flash", display_name="DeepSeek")]
    )

    with patch("routers.model_providers._list_provider_models", list_models):
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
    list_models = AsyncMock(
        return_value=[
            AvailableModel(
                model_id="anthropic.claude-sonnet-4-5", display_name="Claude Sonnet"
            )
        ]
    )

    with (
        patch("routers.model_providers.create_llm_provider", return_value=fake_provider),
        patch("routers.model_providers._list_provider_models", list_models),
    ):
        resp = await admin_client.post(
            "/admin/provider/bedrock/test",
            json={"region_name": "us-east-1"},
        )

    assert resp.status_code == 200
    body = resp.json()
    assert body["ok"] is True
    assert body["model"] == "anthropic.claude-sonnet-4-5"
    list_models.assert_awaited_once()


@pytest.mark.integration
async def test_azure_foundry_without_model_tests_provider_level_connection(admin_client):
    fake_provider = AsyncMock()
    fake_provider.model_name = "gpt-4o"
    fake_provider.provider_type = ProviderType.AZURE_FOUNDRY
    list_models = AsyncMock(
        return_value=[AvailableModel(model_id="gpt-4o", display_name="GPT-4o")]
    )

    with (
        patch("routers.model_providers.create_llm_provider", return_value=fake_provider),
        patch("routers.model_providers._list_provider_models", list_models),
    ):
        resp = await admin_client.post(
            "/admin/provider/azure_foundry/test",
            json={"endpoint_url": "https://example.services.ai.azure.com"},
        )

    assert resp.status_code == 200
    body = resp.json()
    assert body["ok"] is True
    assert body["model"] == "gpt-4o"
    list_models.assert_awaited_once()


@pytest.mark.integration
async def test_vertex_ai_without_model_tests_provider_level_connection(admin_client):
    fake_provider = AsyncMock()
    fake_provider.model_name = "gemini-2.5-flash"
    fake_provider.provider_type = ProviderType.VERTEX_AI
    list_models = AsyncMock(
        return_value=[
            AvailableModel(
                model_id="publishers/google/models/gemini-2.5-flash",
                display_name="Gemini 2.5 Flash",
            )
        ]
    )

    with (
        patch("routers.model_providers.create_llm_provider", return_value=fake_provider),
        patch("routers.model_providers._list_provider_models", list_models),
    ):
        resp = await admin_client.post(
            "/admin/provider/vertex_ai/test",
            json={"region": "us-central1", "project_id": "project"},
        )

    assert resp.status_code == 200
    body = resp.json()
    assert body["ok"] is True
    assert body["model"] == "publishers/google/models/gemini-2.5-flash"
    list_models.assert_awaited_once()


@pytest.mark.integration
async def test_list_provider_models_returns_normalized_models(admin_client):
    fake_provider = AsyncMock()
    fake_provider.provider_type = ProviderType.OPENAI_COMPATIBLE
    discovered_models = AsyncMock(
        return_value=[
            AvailableModel(model_id="deepseek-v4-flash", display_name="DeepSeek v4 Flash"),
            AvailableModel(model_id="deepseek-v4-pro", display_name="DeepSeek v4 Pro"),
        ]
    )

    with (
        patch("routers.model_providers.create_llm_provider", return_value=fake_provider),
        patch("routers.model_providers._list_provider_models", discovered_models),
    ):
        resp = await admin_client.post(
            "/admin/provider/openai_compatible/models",
            json={"base_url": "https://api.deepseek.com", "api_key": "x"},
        )

    assert resp.status_code == 200
    assert resp.json() == {
        "models": [
            {"model_id": "deepseek-v4-flash", "display_name": "DeepSeek v4 Flash"},
            {"model_id": "deepseek-v4-pro", "display_name": "DeepSeek v4 Pro"},
        ]
    }
    discovered_models.assert_awaited_once()
