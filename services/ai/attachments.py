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
import json
import logging
from typing import Literal, TypedDict, cast

import httpx
from anthropic.types import ContentBlockParam, MessageParam, TextBlockParam

from db.uploads import UploadsRepository
from storage import ContentStorage
from tools.document_handler import DocumentToolHandler
from tools.registry import ToolContext


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


def _mention_label(title: str, document_id: str) -> TextBlockParam:
    safe_title = json.dumps(title, ensure_ascii=False)
    return _text_block(f"[Mentioned document: {safe_title}]\n[_ref:{document_id}]")


def _expanded_mention_blocks(
    title: str,
    document_id: str,
    raw: list[ContentBlockParam],
) -> list[ContentBlockParam]:
    blocks: list[ContentBlockParam] = [_mention_label(title, document_id)]
    blocks.extend(raw)
    return blocks


async def _expand_omni_upload(
    upload_id: str,
    chat_id: str,
    storage: ContentStorage,
    uploads_repo: UploadsRepository,
    sandbox_url: str | None,
    cache: dict[UploadId, list[TextBlockParam]],
    user_id: str | None = None,
) -> list[TextBlockParam]:
    if upload_id in cache:
        return cache[upload_id]

    upload = await uploads_repo.get(upload_id)
    if not upload:
        expanded: list[TextBlockParam] = [_text_block(f"[upload {upload_id} not found]")]
        cache[upload_id] = expanded
        return expanded

    if user_id is not None and upload.user_id != user_id:
        expanded: list[TextBlockParam] = [_text_block(f"[upload {upload_id} not found]")]
        cache[upload_id] = expanded
        return expanded

    content = await storage.get_bytes(upload.content_id)

    if _is_textual(upload.content_type):
        try:
            text = content.decode("utf-8")
        except UnicodeDecodeError:
            text = None

        if text is not None and len(text) <= INLINE_TEXT_THRESHOLD:
            expanded = [_text_block(f'<file name="{upload.filename}">\n{text}\n</file>')]
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


class OmniMentionSource(TypedDict):
    type: Literal["omni_mention"]
    document_id: str
    title: str


class OmniMentionBlock(TypedDict):
    type: Literal["document"]
    source: OmniMentionSource


# ID of a document in the `documents` table (ULID). Aliased for self-documenting dict keys.
DocumentId = str


async def _expand_omni_mention(
    document_id: str,
    title: str,
    chat_id: str,
    doc_handler: DocumentToolHandler,
    user_id: str | None,
    user_email: str | None,
    skip_permission_check: bool,
    cache: dict[DocumentId, tuple[bool, list[ContentBlockParam]]],
    user_groups: list[str] | None = None,
) -> list[ContentBlockParam]:
    # cache stores (is_error, content_blocks) — the content blocks are
    # the raw ToolResult content (no label). The label is built fresh
    # per occurrence so each mention gets its own title in the label.
    if document_id in cache:
        is_error, raw = cache[document_id]
        if is_error:
            return [_text_block(f"[Document '{title}' could not be loaded]")]
        return _expanded_mention_blocks(title, document_id, list(raw))

    tool_context = ToolContext(
        chat_id=chat_id,
        user_id=user_id,
        user_email=user_email,
        user_groups=user_groups,
        skip_permission_check=skip_permission_check,
    )

    try:
        result = await doc_handler.execute(
            "read_document",
            {"id": document_id, "name": title},
            tool_context,
        )
    except Exception as e:
        logger.warning(f"expand_mentions: failed to fetch document {document_id}: {e}")
        cache[document_id] = (True, [])
        return [_text_block(f"[Document '{title}' could not be loaded]")]

    if result.is_error:
        logger.info(f"expand_mentions: document {document_id} not accessible")
        cache[document_id] = (True, [])
        return [_text_block(f"[Document '{title}' could not be loaded]")]

    # Cache the raw content (no label). Label is built per occurrence.
    raw_content = list(result.content)
    cache[document_id] = (False, raw_content)
    return _expanded_mention_blocks(title, document_id, raw_content)


def _as_omni_mention(
    block: ContentBlockParam,
) -> OmniMentionBlock | None:
    """Narrow `block` to OmniMentionBlock when it carries a valid omni_mention
    source that should be expanded.

    A valid mention has outer ``type='document'``, a ``source`` dict with
    ``type='omni_mention'``, a nonempty string ``document_id``, and a nonempty
    string ``title``.  Returns None if not a mention or has invalid structure.
    Always safe to call on any block; never raises."""
    if block.get("type") != "document":
        return None
    source = block.get("source")
    if not isinstance(source, dict) or source.get("type") != "omni_mention":
        return None
    document_id = source.get("document_id")
    title = source.get("title")
    if not isinstance(document_id, str) or not document_id:
        return None
    if not isinstance(title, str) or not title:
        return None
    return cast(OmniMentionBlock, block)


def _has_mention_source(block: ContentBlockParam) -> bool:
    """Check if a block carries an omni_mention source dict, regardless of
    outer type or field validity. Used for provider-invariant sanitization:
    any such block must be either validated+expanded (user role) or replaced
    with safe text (non-user)."""
    source = block.get("source")
    return isinstance(source, dict) and source.get("type") == "omni_mention"


async def expand_mentions(
    messages: list[MessageParam],
    chat_id: str,
    doc_handler: DocumentToolHandler,
    user_id: str | None,
    user_email: str | None,
    skip_permission_check: bool = False,
    user_groups: list[str] | None = None,
) -> list[MessageParam]:
    """Return a new message list with all omni_mention blocks expanded.

    Each mention block is replaced by a short label plus the document's
    content (or an error notice if the document isn't accessible).
    The expansion reuses ``DocumentToolHandler.execute("read_document", ...)``
    so text inlining, sandbox staging for large/binary files, and permission
    checks all behave identically to an explicit ``read_document`` tool call.
    """
    cache: dict[DocumentId, tuple[bool, list[ContentBlockParam]]] = {}
    out: list[MessageParam] = []
    for msg in messages:
        # Only expand mentions in user messages. Assistant messages carry
        # mention references only via tool_use input params, not as top-level
        # content blocks, so they should never match _as_omni_mention. Guard
        # here to stay safe against accidental expansion.
        if msg["role"] != "user":
            # Sanitize any omni_mention blocks found outside user role.
            # Never pass custom blocks through to the provider.
            content = msg["content"]
            if isinstance(content, list):
                new_blocks: list[ContentBlockParam] = []
                changed = False
                for block in content:
                    if _has_mention_source(block):
                        new_blocks.append(_text_block("[Invalid document mention omitted]"))
                        changed = True
                    else:
                        new_blocks.append(block)
                if changed:
                    out.append({**msg, "content": new_blocks})
                else:
                    out.append(msg)
            else:
                out.append(msg)
            continue
        content = msg["content"]
        if not isinstance(content, list):
            out.append(msg)
            continue

        new_blocks: list[ContentBlockParam] = []
        changed = False
        for block in content:
            mention_block = _as_omni_mention(block)
            if mention_block is not None:
                new_blocks.extend(
                    await _expand_omni_mention(
                        mention_block["source"]["document_id"],
                        mention_block["source"]["title"],
                        chat_id,
                        doc_handler,
                        user_id,
                        user_email,
                        skip_permission_check,
                        cache,
                        user_groups=user_groups,
                    )
                )
                changed = True
            elif _has_mention_source(block):
                # Invalid mention structure in user message — replace with
                # safe text rather than passing a malformed custom block.
                new_blocks.append(_text_block("[Invalid document mention omitted]"))
                changed = True
            else:
                new_blocks.append(block)

        if changed:
            out.append({**msg, "content": new_blocks})
        else:
            out.append(msg)

    return out


async def expand_uploads(
    messages: list[MessageParam],
    chat_id: str,
    storage: ContentStorage,
    uploads_repo: UploadsRepository,
    sandbox_url: str | None,
    user_id: str | None = None,
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
                    user_id=user_id,
                )
            )
            changed = True

        if changed:
            out.append({**msg, "content": new_blocks})
        else:
            out.append(msg)

    return out
