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

from anthropic.types import TextBlockParam

logger = logging.getLogger(__name__)


class OmniToolResultKind(str, Enum):
    OAUTH_REQUIRED = "oauth_required"
    APPROVAL_REQUIRED = "approval_required"


@dataclass
class ApprovalRequiredPayload:
    approval_id: str
    tool_name: str
    tool_input: dict[str, object]
    tool_call_id: str
    source_id: str | None = None
    source_type: str | None = None


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


def _encode_envelope(kind: OmniToolResultKind, payload: object) -> TextBlockParam:
    return TextBlockParam(
        type="text",
        text=json.dumps(
            {
                "omni_kind": kind.value,
                "payload": asdict(payload),
            }
        ),
    )


def encode_approval_required(payload: ApprovalRequiredPayload) -> TextBlockParam:
    """Wrap an ApprovalRequiredPayload as a tool_result text content block."""
    return _encode_envelope(OmniToolResultKind.APPROVAL_REQUIRED, payload)


def encode_oauth_required(payload: OAuthRequiredPayload) -> TextBlockParam:
    """Wrap an OAuthRequiredPayload as a tool_result text content block."""
    return _encode_envelope(OmniToolResultKind.OAUTH_REQUIRED, payload)
