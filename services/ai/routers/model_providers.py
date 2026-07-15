import asyncio
import datetime
import logging
import re
import time

from fastapi import APIRouter, Request
from pydantic import BaseModel

from db import ModelsRepository
from providers import LLMProvider, ProviderError, ProviderType, create_llm_provider
from providers.anthropic import AnthropicProvider
from providers.azure_foundry import AzureFoundryProvider
from providers.bedrock import BedrockProvider
from providers.gemini import GeminiProvider
from providers.openai import OpenAIProvider
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


class AvailableModel(BaseModel):
    model_id: str
    display_name: str


class ListProviderModelsResponse(BaseModel):
    models: list[AvailableModel]


DISCOVERED_MODELS_LIMIT = 3


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


def _display_name(model_id: str, candidate: object = None) -> str:
    return candidate if isinstance(candidate, str) and candidate else model_id


def _google_model_id(name: object) -> str | None:
    if not isinstance(name, str) or not name:
        return None
    if "/models/" in name:
        return name.rsplit("/models/", 1)[1]
    if name.startswith("models/"):
        return name.removeprefix("models/")
    return name


def _is_openai_chat_model(model_id: str) -> bool:
    normalized = model_id.lower()
    if any(
        part in normalized
        for part in (
            "audio",
            "dall-e",
            "embedding",
            "image",
            "moderation",
            "realtime",
            "search",
            "tts",
            "transcribe",
            "whisper",
        )
    ):
        return False
    return normalized.startswith(("gpt-", "chatgpt-", "o1", "o3", "o4", "o5"))


async def _list_openai_sdk_models(
    client: object,
    *,
    limit: int = DISCOVERED_MODELS_LIMIT,
    chat_only: bool = False,
) -> list[AvailableModel]:
    models_page = await client.models.list()
    raw = getattr(models_page, "data", None) or []
    # Filter to chat-capable models
    filtered = []
    for m in raw:
        model_id = getattr(m, "id", None)
        if not isinstance(model_id, str) or not model_id:
            continue
        if chat_only and not _is_openai_chat_model(model_id):
            continue
        filtered.append(m)
    # Sort by creation time descending (newest first)
    filtered.sort(key=lambda m: getattr(m, "created", 0) or 0, reverse=True)
    return [
        AvailableModel(model_id=getattr(m, "id", ""), display_name=getattr(m, "id", ""))
        for m in filtered[:limit]
    ]


async def _list_anthropic_models(
    provider: AnthropicProvider,
    limit: int = DISCOVERED_MODELS_LIMIT,
) -> list[AvailableModel]:
    models_page = await provider.client.models.list()
    raw = getattr(models_page, "data", None) or []
    # Sort by creation time descending (newest first).
    # Anthropic's API already returns newest-first, but this is defensive.
    raw.sort(
        key=lambda m: getattr(m, "created_at", datetime.datetime.min),
        reverse=True,
    )
    return [
        AvailableModel(
            model_id=getattr(m, "id", ""),
            display_name=_display_name(
                getattr(m, "id", ""), getattr(m, "display_name", None)
            ),
        )
        for m in raw[:limit]
    ]


# Latest Gemini models as of July 2026.
_GEMINI_LATEST_MODELS = [
    ("gemini-3.5-flash", "Gemini 3.5 Flash"),
    ("gemini-3.5-pro", "Gemini 3.5 Pro"),
    ("gemini-3.1-flash-lite", "Gemini 3.1 Flash Lite"),
]


async def _list_gemini_models(
    provider: GeminiProvider | VertexAIProvider,
    limit: int = DISCOVERED_MODELS_LIMIT,
) -> list[AvailableModel]:
    return [
        AvailableModel(model_id=mid, display_name=dname)
        for mid, dname in _GEMINI_LATEST_MODELS[:limit]
    ]


async def _list_bedrock_models(
    req: TestModelRequest,
    limit: int = DISCOVERED_MODELS_LIMIT,
) -> list[AvailableModel]:
    import boto3

    client = boto3.client("bedrock", region_name=req.region_name)
    response = await asyncio.to_thread(client.list_foundation_models)
    summaries = response.get("modelSummaries", [])
    entries: list[tuple[AvailableModel, object]] = []
    for summary in summaries:
        model_id = summary.get("modelId")
        if not isinstance(model_id, str) or not model_id:
            continue
        if not any(family in model_id.lower() for family in BedrockProvider.MODEL_FAMILIES):
            continue
        lifecycle = summary.get("modelLifecycle") or {}
        start_time = lifecycle.get("startOfLifeTime") or datetime.datetime.min
        entries.append(
            (
                AvailableModel(
                    model_id=model_id,
                    display_name=_display_name(model_id, summary.get("modelName")),
                ),
                start_time,
            )
        )
    entries.sort(key=lambda e: e[1], reverse=True)
    return [e[0] for e in entries[:limit]]


async def _list_provider_models(
    provider_type: ProviderType,
    provider: LLMProvider,
    req: TestModelRequest,
    limit: int = DISCOVERED_MODELS_LIMIT,
) -> list[AvailableModel]:
    if provider_type == ProviderType.ANTHROPIC and isinstance(provider, AnthropicProvider):
        return await _list_anthropic_models(provider, limit)
    if provider_type == ProviderType.OPENAI and isinstance(provider, OpenAIProvider):
        return await _list_openai_sdk_models(provider.client, limit=limit, chat_only=True)
    if provider_type == ProviderType.GEMINI and isinstance(provider, GeminiProvider):
        return await _list_gemini_models(provider, limit)
    if provider_type == ProviderType.OPENAI_COMPATIBLE and isinstance(
        provider, OpenAICompatibleProvider
    ):
        return await _list_openai_sdk_models(provider.client, limit=limit)
    if provider_type == ProviderType.BEDROCK:
        return await _list_bedrock_models(req, limit)
    if provider_type == ProviderType.AZURE_FOUNDRY and isinstance(
        provider, AzureFoundryProvider
    ):
        return await _list_openai_sdk_models(provider._delegate.client, limit=limit)
    if provider_type == ProviderType.VERTEX_AI and isinstance(provider, VertexAIProvider):
        return await _list_gemini_models(provider, limit)
    return []


async def _first_provider_model(
    provider_type: ProviderType,
    provider: LLMProvider,
    req: TestModelRequest,
) -> str | None:
    models = await _list_provider_models(provider_type, provider, req, limit=1)
    return models[0].model_id if models else None


@router.post(
    "/admin/provider/{provider_type}/models", response_model=ListProviderModelsResponse
)
async def list_provider_models(
    provider_type: ProviderType,
    req: TestModelRequest,
) -> ListProviderModelsResponse:
    try:
        provider = _build_provider(provider_type, req)
        models = await asyncio.wait_for(
            _list_provider_models(provider_type, provider, req),
            timeout=15,
        )
        return ListProviderModelsResponse(models=models)
    except Exception as e:
        logger.warning(f"List models: failed for {provider_type}: {e}")
        return ListProviderModelsResponse(models=[])


@router.post("/admin/provider/{provider_type}/test", response_model=TestModelResponse)
async def test_model_provider(
    provider_type: ProviderType,
    req: TestModelRequest,
) -> TestModelResponse:
    """List provider models to validate a not-necessarily-saved provider config.
    Used by the admin Test Connection button."""
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
        models = await asyncio.wait_for(
            _list_provider_models(provider_type, provider, req, limit=1),
            timeout=15,
        )
        latency = int((time.monotonic() - start) * 1000)
        return TestModelResponse(
            ok=True,
            provider=provider.provider_type,
            model=models[0].model_id if models else None,
            latency_ms=latency,
        )
    except ProviderError as e:
        return TestModelResponse(
            ok=False,
            error=e.message,
            provider=e.provider_type,
            status_code=e.status_code,
            model=None,
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
            model=None,
        )
