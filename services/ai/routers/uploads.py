"""Uploads router: stores ad-hoc user files via the configured content storage backend.

The web service proxies authenticated multipart uploads here. The user_id is supplied by
the web layer (which has performed auth) as a form field.
"""

import logging

import ulid
from fastapi import APIRouter, File, Form, HTTPException, Request, UploadFile

from db.uploads import UploadsRepository

logger = logging.getLogger(__name__)

router = APIRouter(tags=["uploads"], prefix="/uploads")

MAX_UPLOAD_BYTES = 50 * 1024 * 1024  # 50 MB


@router.post("")
async def create_upload(
    request: Request,
    user_id: str = Form(...),
    file: UploadFile = File(...),
):
    storage = request.app.state.content_storage
    if storage is None:
        raise HTTPException(status_code=503, detail="Content storage not initialized")

    data = await file.read()
    if len(data) == 0:
        raise HTTPException(status_code=400, detail="Empty file")
    if len(data) > MAX_UPLOAD_BYTES:
        raise HTTPException(status_code=413, detail="File too large")

    content_type = file.content_type or "application/octet-stream"
    filename = file.filename or "upload"

    content_id = await storage.put(data, content_type)

    upload_id = str(ulid.ULID())
    upload = await UploadsRepository().create(
        upload_id=upload_id,
        user_id=user_id,
        content_id=content_id,
        filename=filename,
        content_type=content_type,
        size_bytes=len(data),
    )

    return {
        "id": upload.id,
        "filename": upload.filename,
        "content_type": upload.content_type,
        "size_bytes": upload.size_bytes,
        "created_at": upload.created_at.isoformat(),
    }


@router.get("/{upload_id}")
async def get_upload(upload_id: str):
    upload = await UploadsRepository().get(upload_id)
    if not upload:
        raise HTTPException(status_code=404, detail="Upload not found")
    return {
        "id": upload.id,
        "user_id": upload.user_id,
        "filename": upload.filename,
        "content_type": upload.content_type,
        "size_bytes": upload.size_bytes,
        "created_at": upload.created_at.isoformat(),
    }
