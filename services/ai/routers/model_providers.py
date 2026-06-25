import asyncio
import logging
import time

from fastapi import APIRouter, Request
from pydantic import BaseModel

from db import ModelsRepository
from providers import LLMProvider, ProviderError, ProviderType, create_llm_provider
from providers.azure_foundry import AzureFoundryProvider
from providers.openai_compatible import OpenAICompatibleProvider
from providers.vertex_ai import VertexAIProvider
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


def _error_status_code(e: BaseException) -> int | None:
    status_code = getattr(e, "status_code", None)
    if isinstance(status_code, int):
        return status_code

    response = getattr(e, "response", None)
    if isinstance(response, dict):
        metadata = response.get("ResponseMetadata")
        if isinstance(metadata, dict):
            aws_status_code = metadata.get("HTTPStatusCode")
            if isinstance(aws_status_code, int):
                return aws_status_code

    return None


async def _test_openai_compatible_connection_without_model(
    provider: OpenAICompatibleProvider,
) -> str | None:
    models_page = await provider.client.models.list()
    models = getattr(models_page, "data", None) or []
    if not models:
        return None
    first_model = models[0]
    model_id = getattr(first_model, "id", None)
    return model_id if isinstance(model_id, str) else None


async def _test_bedrock_connection_without_model(req: TestModelRequest) -> str | None:
    import boto3

    client = boto3.client("bedrock", region_name=req.region_name)
    response = await asyncio.to_thread(client.list_foundation_models)
    summaries = response.get("modelSummaries", [])
    if not summaries:
        return None
    model_id = summaries[0].get("modelId")
    return model_id if isinstance(model_id, str) else None


async def _test_azure_foundry_connection_without_model(
    provider: AzureFoundryProvider,
) -> str | None:
    delegate = provider._delegate
    client = delegate.client
    models_page = await client.models.list()
    models = getattr(models_page, "data", None) or []
    if not models:
        return None
    first_model = models[0]
    model_id = getattr(first_model, "id", None)
    return model_id if isinstance(model_id, str) else None


async def _test_vertex_ai_connection_without_model(
    provider: VertexAIProvider,
) -> str | None:
    delegate = provider._delegate
    client = delegate.client
    async_models = client.aio.models.list()
    async for model in async_models:
        model_name = getattr(model, "name", None)
        if isinstance(model_name, str):
            return model_name
        model_id = getattr(model, "id", None)
        if isinstance(model_id, str):
            return model_id
    return None


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
        no_model = req.model is None and req.model_id is None
        provider_level_model: str | None = None
        if no_model:
            if provider_type == ProviderType.OPENAI_COMPATIBLE:
                provider_level_model = await asyncio.wait_for(
                    _test_openai_compatible_connection_without_model(provider),
                    timeout=15,
                )
            elif provider_type == ProviderType.BEDROCK:
                provider_level_model = await asyncio.wait_for(
                    _test_bedrock_connection_without_model(req),
                    timeout=15,
                )
            elif provider_type == ProviderType.AZURE_FOUNDRY:
                provider_level_model = await asyncio.wait_for(
                    _test_azure_foundry_connection_without_model(provider),
                    timeout=15,
                )
            elif provider_type == ProviderType.VERTEX_AI:
                provider_level_model = await asyncio.wait_for(
                    _test_vertex_ai_connection_without_model(provider),
                    timeout=15,
                )

            if provider_level_model is not None or provider_type in {
                ProviderType.OPENAI_COMPATIBLE,
                ProviderType.BEDROCK,
                ProviderType.AZURE_FOUNDRY,
                ProviderType.VERTEX_AI,
            }:
                latency = int((time.monotonic() - start) * 1000)
                return TestModelResponse(
                    ok=True,
                    provider=provider.provider_type,
                    model=provider_level_model,
                    latency_ms=latency,
                )

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
    except TimeoutError:
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
            status_code=_error_status_code(e),
            model=provider.model_name,
        )
