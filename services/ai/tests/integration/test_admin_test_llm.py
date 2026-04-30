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
