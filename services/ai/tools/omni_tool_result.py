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


def encode_oauth_required(payload: OAuthRequiredPayload) -> TextBlockParam:
    """Wrap an OAuthRequiredPayload as a tool_result text content block."""
    return TextBlockParam(
        type="text",
        text=json.dumps(
            {
                "omni_kind": OmniToolResultKind.OAUTH_REQUIRED.value,
                "payload": asdict(payload),
            }
        ),
    )
