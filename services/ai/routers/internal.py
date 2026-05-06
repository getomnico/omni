"""Internal endpoints — for service-to-service calls only, not browser-exposed."""

import logging

from fastapi import APIRouter

logger = logging.getLogger(__name__)
router = APIRouter(prefix="/internal", tags=["internal"])
