"""Typed envelopes for structured tool_result content blocks.

When a tool needs to surface a UI-driven prompt (e.g. "user must complete OAuth")
rather than a normal action result, we encode a typed envelope inside the
tool_result's text content. The frontend parses this envelope and renders the
appropriate prompt; if the LLM ever sees the placeholder it sees machine-readable
JSON rather than misleading prose.

Anthropic's tool_result content array only accepts `text` and `image` block
types — JSON-in-text is the cleanest portable shape for structured signals.
"""

from __future__ import annotations

import json
import logging
from dataclasses import asdict, dataclass
from enum import Enum
from typing import TypedDict

logger = logging.getLogger(__name__)


class OmniToolResultKind(str, Enum):
    OAUTH_REQUIRED = "oauth_required"


@dataclass
class OAuthRequiredPayload:
    """Surfaced when a connector action returns 412 needs_user_auth.

    `oauth_start_url` is relative (e.g. `/api/oauth/start?source_id=...`); the
    web layer prefixes the host before opening the popup.
    """

    source_id: str
    source_type: str
    provider: str
    oauth_start_url: str


class _TextBlock(TypedDict):
    type: str
    text: str


def encode_oauth_required(payload: OAuthRequiredPayload) -> _TextBlock:
    """Wrap an OAuthRequiredPayload as a tool_result text content block."""
    return {
        "type": "text",
        "text": json.dumps(
            {
                "omni_kind": OmniToolResultKind.OAUTH_REQUIRED.value,
                "payload": asdict(payload),
            }
        ),
    }


def try_parse_envelope(text: str) -> tuple[OmniToolResultKind, dict] | None:
    """Best-effort parse of a tool_result text block as an Omni envelope.

    Returns (kind, raw_payload_dict) on success, None if the text isn't a valid
    envelope. Server-side this is rarely needed (the AI service mints these
    envelopes via encode_*), but it's useful for the resume path which reads
    persisted tool_result content back out of chat_messages.
    """
    if not text or not text.startswith("{"):
        return None
    try:
        obj = json.loads(text)
    except (ValueError, TypeError):
        return None
    if not isinstance(obj, dict):
        return None
    raw_kind = obj.get("omni_kind")
    raw_payload = obj.get("payload")
    if not isinstance(raw_kind, str) or not isinstance(raw_payload, dict):
        return None
    try:
        kind = OmniToolResultKind(raw_kind)
    except ValueError:
        return None
    return kind, raw_payload
