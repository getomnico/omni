"""Unit tests for omni_upload block expansion (attachments.py)."""

from __future__ import annotations

import base64

import pytest

from attachments import expand_uploads


class FakeUpload:
    def __init__(
        self,
        upload_id: str,
        content_type: str,
        filename: str = "file",
        user_id: str = "user-1",
        size_bytes: int = 0,
    ):
        self.id = upload_id
        self.content_id = f"content-{upload_id}"
        self.content_type = content_type
        self.filename = filename
        self.user_id = user_id
        self.size_bytes = size_bytes


class FakeUploadsRepository:
    def __init__(self, uploads: list[FakeUpload]):
        self._uploads = {u.id: u for u in uploads}

    async def get(self, upload_id: str) -> FakeUpload | None:
        return self._uploads.get(upload_id)


class FakeStorage:
    def __init__(self, blobs: dict[str, bytes]):
        self._blobs = blobs

    async def get_bytes(self, content_id: str) -> bytes:
        return self._blobs[content_id]


PNG_BYTES = b"\x89PNG\r\n\x1a\nfakepng"


def _png_upload(upload_id: str = "upload-1", size_bytes: int = len(PNG_BYTES)):
    return (
        FakeUpload(upload_id, "image/png", filename="shot.png", size_bytes=size_bytes),
        {f"content-{upload_id}": PNG_BYTES},
    )



@pytest.mark.asyncio
async def test_image_upload_expands_to_base64_image_block():
    upload, blobs = _png_upload()
    messages = [
        {
            "role": "user",
            "content": [
                {"type": "image", "source": {"type": "omni_upload", "upload_id": "upload-1"}},
                {"type": "text", "text": "what is this?"},
            ],
        }
    ]

    out = await expand_uploads(
        messages, "chat-1", FakeStorage(blobs), FakeUploadsRepository([upload]), None
    )

    blocks = out[0]["content"]
    assert blocks[0]["type"] == "image"
    assert blocks[0]["source"]["type"] == "base64"
    assert blocks[0]["source"]["media_type"] == "image/png"
    assert base64.b64decode(blocks[0]["source"]["data"]) == PNG_BYTES
    assert blocks[1] == {"type": "text", "text": "what is this?"}


@pytest.mark.asyncio
async def test_oversized_image_falls_back_to_pointer_text():
    upload, blobs = _png_upload()
    upload.size_bytes = 10_000_000
    blobs[f"content-{upload.id}"] = b"x" * 10_000_000
    messages = [
        {
            "role": "user",
            "content": [
                {"type": "image", "source": {"type": "omni_upload", "upload_id": "upload-1"}}
            ],
        }
    ]

    out = await expand_uploads(
        messages, "chat-1", FakeStorage(blobs), FakeUploadsRepository([upload]), None
    )

    blocks = out[0]["content"]
    assert blocks[0]["type"] == "text"
    assert "too large to inline" in blocks[0]["text"]


@pytest.mark.asyncio
async def test_unsupported_image_media_type_falls_back_to_pointer_text():
    upload = FakeUpload("upload-1", "image/heic", filename="photo.heic")
    blobs = {"content-upload-1": b"heic-bytes"}
    messages = [
        {
            "role": "user",
            "content": [
                {"type": "image", "source": {"type": "omni_upload", "upload_id": "upload-1"}}
            ],
        }
    ]

    out = await expand_uploads(
        messages, "chat-1", FakeStorage(blobs), FakeUploadsRepository([upload]), None
    )

    blocks = out[0]["content"]
    assert blocks[0]["type"] == "text"
    assert "Available in workspace at" not in blocks[0]["text"]
    assert "no sandbox is available" in blocks[0]["text"]


@pytest.mark.asyncio
async def test_text_upload_still_inlines_as_text():
    upload = FakeUpload("upload-1", "text/plain", filename="notes.txt")
    blobs = {"content-upload-1": b"hello world"}
    messages = [
        {
            "role": "user",
            "content": [
                {
                    "type": "document",
                    "source": {"type": "omni_upload", "upload_id": "upload-1"},
                }
            ],
        }
    ]

    out = await expand_uploads(
        messages, "chat-1", FakeStorage(blobs), FakeUploadsRepository([upload]), None
    )

    blocks = out[0]["content"]
    assert blocks[0]["type"] == "text"
    assert '<file name="notes.txt">' in blocks[0]["text"]
    assert "hello world" in blocks[0]["text"]


@pytest.mark.asyncio
async def test_upload_of_other_user_is_not_visible():
    upload, blobs = _png_upload()
    upload.user_id = "someone-else"
    messages = [
        {
            "role": "user",
            "content": [
                {"type": "image", "source": {"type": "omni_upload", "upload_id": "upload-1"}}
            ],
        }
    ]

    out = await expand_uploads(
        messages, "chat-1", FakeStorage(blobs), FakeUploadsRepository([upload]), None,
        user_id="user-1",
    )

    blocks = out[0]["content"]
    assert blocks[0] == {"type": "text", "text": "[upload upload-1 not found]"}
