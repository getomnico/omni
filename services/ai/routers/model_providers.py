import asyncio
import logging
import time
from fastapi import APIRouter, Request
from pydantic import BaseModel

from db import ModelsRepository
from providers import LLMProvider, ProviderError, ProviderType, create_llm_provider
from services.providers import load_models

router = APIRouter(tags=["model-providers"])
logger = logging.getLogger(__name__)


@router.get("/models")
async def list_models():
    """Return active models (no secrets)."""
    repo = ModelsRepository()
    records = await repo.list_active()
    return [
        {
            "id": r.id,
            "modelId": r.model_id,
            "displayName": r.display_name,
            "providerType": r.provider_type,
            "isDefault": r.is_default,
        }
        for r in records
    ]


@router.post("/admin/reload-providers")
async def reload_providers(request: Request):
    """Reload model instances from the database."""
    await load_models(request.app.state)
    return {"status": "ok"}


class TestModelRequest(BaseModel):
    api_key: str | None = None
    model: str | None = None
    region_name: str | None = None
    model_id: str | None = None
    region: str | None = None
    project_id: str | None = None
    endpoint_url: str | None = None
    base_url: str | None = None


class TestModelResponse(BaseModel):
    ok: bool
    error: str | None = None
    provider: ProviderType | None = None
    status_code: int | None = None
    model: str | None = None
    latency_ms: int | None = None


def _build_provider(provider_type: ProviderType, req: TestModelRequest) -> LLMProvider:
    kwargs = req.model_dump(exclude_none=True)
    return create_llm_provider(provider_type, **kwargs)


@router.post("/admin/provider/{provider_type}/test", response_model=TestModelResponse)
async def test_model_provider(
    provider_type: ProviderType,
    req: TestModelRequest,
) -> TestModelResponse:
    """Send a tiny prompt to a not-necessarily-saved provider config and surface
    the real error if it fails. Used by the admin Test Connection button."""
    try:
        provider = _build_provider(provider_type, req)
    except Exception as e:
        logger.warning(f"Test connection: failed to instantiate {provider_type}: {e}")
        return TestModelResponse(
            ok=False,
            error=f"Configuration error: {e}",
            provider=provider_type,
        )

    start = time.monotonic()
    try:
        await asyncio.wait_for(
            provider.generate_response("Hi", max_tokens=5),
            timeout=15,
        )
        latency = int((time.monotonic() - start) * 1000)
        return TestModelResponse(
            ok=True,
            provider=provider.provider_type,
            model=provider.model_name,
            latency_ms=latency,
        )
    except ProviderError as e:
        return TestModelResponse(
            ok=False,
            error=e.message,
            provider=e.provider_type,
            status_code=e.status_code,
            model=e.model,
        )
    except asyncio.TimeoutError:
        return TestModelResponse(
            ok=False,
            error="Test timed out after 15s",
            provider=provider.provider_type,
            model=provider.model_name,
        )
    except Exception as e:
        logger.warning(f"Test connection: unexpected error for {provider_type}: {e}")
        return TestModelResponse(
            ok=False,
            error=str(e),
            provider=provider.provider_type,
            model=provider.model_name,
        )
