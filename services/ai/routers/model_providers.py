import logging

from fastapi import APIRouter, Request

from db import ModelsRepository
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
