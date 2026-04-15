"""Expand omni_upload content blocks in user messages before sending to the LLM.

User messages may carry blocks shaped like::

    {"type": "document"|"image", "source": {"type": "omni_upload", "upload_id": "..."}}

These are persisted as-is (compact, replayable). At provider-call time we expand them:
- text upload <= 32KB  -> inline as a text block
- otherwise            -> stage in /scratch/{chat_id}/<upload_id>_<filename> and emit a
                          short text pointer block telling the model the file is in the
                          workspace (model can then use read_file / run_bash / run_python).

The sandbox path is content-addressable on upload_id, so re-staging across turns is a
single existence check and a no-op when the file is already there.
"""

from __future__ import annotations

import base64
import logging

from typing import Literal, TypedDict, cast

import httpx
from anthropic.types import ContentBlockParam, MessageParam, TextBlockParam

from db.uploads import UploadsRepository
from storage import ContentStorage


# Our custom source variant embedded in Anthropic document/image blocks. Not part of
# Anthropic's source union — resolved to real content blocks by `expand_uploads`.
class OmniUploadSource(TypedDict):
    type: Literal["omni_upload"]
    upload_id: str


class OmniUploadBlock(TypedDict):
    type: Literal["document", "image"]
    source: OmniUploadSource


# ID of a row in the `uploads` table (ULID). Aliased for self-documenting dict keys.
UploadId = str

logger = logging.getLogger(__name__)

INLINE_TEXT_THRESHOLD = 32_000  # characters

# Content types we treat as text and try to inline when small enough.
_TEXT_PREFIXES = ("text/",)
_TEXT_EXTRAS = {
    "application/json",
    "application/xml",
    "application/x-yaml",
    "application/yaml",
    "application/javascript",
    "application/sql",
}


def _is_textual(content_type: str) -> bool:
    ct = (content_type or "").lower()
    return ct.startswith(_TEXT_PREFIXES) or ct in _TEXT_EXTRAS


def _sandbox_path(upload_id: str, filename: str) -> str:
    safe = filename.replace("/", "_").replace("\\", "_")
    return f"{upload_id}_{safe}"


async def _stage_in_sandbox(
    sandbox_url: str,
    chat_id: str,
    path: str,
    content: bytes,
) -> None:
    """Write `content` to the sandbox at `path`, skipping if a file already exists there."""
    base = sandbox_url.rstrip("/")
    async with httpx.AsyncClient(timeout=60.0) as client:
        stat = await client.post(
            f"{base}/files/stat",
            json={"path": path, "chat_id": chat_id},
        )
        stat.raise_for_status()
        if stat.json().get("exists"):
            return

        encoded = base64.b64encode(content).decode("ascii")
        write = await client.post(
            f"{base}/files/write_binary",
            json={
                "path": path,
                "content_base64": encoded,
                "chat_id": chat_id,
            },
        )
        write.raise_for_status()


def _text_block(text: str) -> TextBlockParam:
    return {"type": "text", "text": text}


async def _expand_omni_upload(
    upload_id: str,
    chat_id: str,
    storage: ContentStorage,
    uploads_repo: UploadsRepository,
    sandbox_url: str | None,
    cache: dict[UploadId, list[TextBlockParam]],
) -> list[TextBlockParam]:
    if upload_id in cache:
        return cache[upload_id]

    upload = await uploads_repo.get(upload_id)
    if not upload:
        expanded: list[TextBlockParam] = [
            _text_block(f"[upload {upload_id} not found]")
        ]
        cache[upload_id] = expanded
        return expanded

    content = await storage.get_bytes(upload.content_id)

    if _is_textual(upload.content_type):
        try:
            text = content.decode("utf-8")
        except UnicodeDecodeError:
            text = None

        if text is not None and len(text) <= INLINE_TEXT_THRESHOLD:
            expanded = [
                _text_block(f'<file name="{upload.filename}">\n{text}\n</file>')
            ]
            cache[upload_id] = expanded
            return expanded

    if not sandbox_url:
        expanded = [
            _text_block(
                f"[uploaded file '{upload.filename}' "
                f"({upload.content_type}, {upload.size_bytes} bytes) "
                f"is too large to inline and no sandbox is available]"
            )
        ]
        cache[upload_id] = expanded
        return expanded

    path = _sandbox_path(upload_id, upload.filename)
    await _stage_in_sandbox(sandbox_url, chat_id, path, content)

    expanded = [
        _text_block(
            f"User attached file '{upload.filename}' "
            f"({upload.content_type}, {upload.size_bytes} bytes). "
            f"Available in workspace at '{path}'. "
            f"Use read_file, run_bash, or run_python to inspect it."
        )
    ]
    cache[upload_id] = expanded
    return expanded


def _as_omni_upload(block: ContentBlockParam) -> OmniUploadBlock | None:
    """Narrow `block` to OmniUploadBlock when it carries an omni_upload source."""
    if block["type"] != "document" and block["type"] != "image":
        return None
    # Document/image carry `source`. Our omni_upload variant isn't in Anthropic's union
    # so we inspect it as a plain mapping before narrowing.
    source = cast(dict, block).get("source")
    if not isinstance(source, dict) or source.get("type") != "omni_upload":
        return None
    if not isinstance(source.get("upload_id"), str):
        return None
    return cast(OmniUploadBlock, block)


async def expand_uploads(
    messages: list[MessageParam],
    chat_id: str,
    storage: ContentStorage,
    uploads_repo: UploadsRepository,
    sandbox_url: str | None,
) -> list[MessageParam]:
    """Return a new message list with all omni_upload blocks expanded.

    Cheap to call every turn: deterministic per upload_id, with an in-call cache and a
    sandbox stat-before-write to avoid re-uploading staged files.
    """
    cache: dict[UploadId, list[TextBlockParam]] = {}
    out: list[MessageParam] = []
    for msg in messages:
        content = msg["content"]
        if not isinstance(content, list):
            out.append(msg)
            continue

        new_blocks: list[ContentBlockParam] = []
        changed = False
        for block in content:
            upload_block = _as_omni_upload(block)
            if upload_block is None:
                new_blocks.append(block)
                continue

            new_blocks.extend(
                await _expand_omni_upload(
                    upload_block["source"]["upload_id"],
                    chat_id,
                    storage,
                    uploads_repo,
                    sandbox_url,
                    cache,
                )
            )
            changed = True

        if changed:
            out.append({**msg, "content": new_blocks})
        else:
            out.append(msg)

    return out
