import logging
from dataclasses import asdict

from fastapi import APIRouter, Query

from db.usage import UsageRepository

logger = logging.getLogger(__name__)

router = APIRouter(tags=["usage"])


@router.get("/usage/summary")
async def get_usage_summary(
    days: int = Query(30, ge=1, le=365),
    user_id: str | None = Query(None),
):
    """Return aggregated token usage summary grouped by model, provider, and purpose."""
    repo = UsageRepository()
    summary = await repo.get_summary(days=days, user_id=user_id)
    return {"period_days": days, "usage": [asdict(row) for row in summary]}
