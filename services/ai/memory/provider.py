"""Provider-agnostic memory interface, namespace key shapes, and mode resolution.

Callers in routers/chat.py, routers/memory.py and agents/executor.py
type against `MemoryProvider`, not against the concrete mem0 wrapper.
This keeps the rest of the AI service insulated from which backend is
in use, so swapping mem0 for a native ParadeDB implementation later is
a config change, not a refactor of every call site.
"""

import logging
from dataclasses import dataclass, field
from datetime import datetime
from enum import IntEnum
from typing import Any, Protocol

from anthropic.types import MessageParam

logger = logging.getLogger(__name__)


# ---------------------------------------------------------------------------
# Memory mode
# ---------------------------------------------------------------------------


class MemoryMode(IntEnum):
    """Effective memory mode for a request.

    Levels (ordered by capability so int comparison gives ceiling logic):
      - OFF: no reads, no writes. Memory is invisible to the user and to
        the model.
      - CHAT: writes are enabled — completed assistant turns get persisted
        and used to inform future replies (the LLM-extracted facts surface
        in the system prompt for the next turn). Manual viewing/deletion
        from the settings UI also works at this level.
      - FULL: everything in CHAT, plus the agent loop can call the
        `manage_memory` tool to add/update/remove memories mid-run.
    """

    OFF = 0
    CHAT = 1
    FULL = 2

    @classmethod
    def parse(cls, raw: str | None) -> "MemoryMode | None":
        """Parse a raw string into a MemoryMode.

        Returns None for `None` / empty input. Returns None for unknown
        values (caller decides whether that's a hard error or a silent
        fall-back to OFF).
        """
        if not raw:
            return None
        try:
            return cls[raw.upper()]
        except KeyError:
            logger.warning(f"Unknown memory mode: {raw!r}")
            return None


def parse_org_default(config_value: dict | None) -> MemoryMode:
    """Parse the `memory_mode_default` preferences row into a MemoryMode.

    The preferences table stores `{"value": "<mode>"}` (admin UI) or
    `{"mode": "<mode>"}` (legacy migration seed); accept either. Anything
    else is treated as OFF.
    """
    if not config_value:
        return MemoryMode.OFF
    raw = config_value.get("value") or config_value.get("mode")
    return MemoryMode.parse(raw) or MemoryMode.OFF


def resolve_memory_mode(
    user_mode: MemoryMode | None,
    org_default: MemoryMode,
) -> MemoryMode:
    """Return the effective memory mode for a request.

    Org default is a **ceiling**, not just a fallback: users can never exceed
    the mode the org admin has enabled. If the user has no override they
    inherit `org_default`; if they pick a higher mode it is capped down.
    """
    if user_mode is None:
        return org_default
    return min(user_mode, org_default)


# ---------------------------------------------------------------------------
# Namespace keys
# ---------------------------------------------------------------------------


def user_key(user_id: str) -> str:
    """Namespace for chat memory belonging to a user."""
    return f"user:{user_id}"


def agent_key(agent_id: str) -> str:
    """Namespace for memory belonging to a background agent.

    Both org agents and personal agents use this — the agent id is
    unique across the agents table, so there's no need to encode the
    agent type or owner in the key.
    """
    return f"agent:{agent_id}"


# ---------------------------------------------------------------------------
# Provider interface
# ---------------------------------------------------------------------------


@dataclass(frozen=True)
class MemoryRecord:
    """A persisted memory — what the provider actually stores.

    `text` is the LLM-consumable surface every consumer ultimately injects
    into a system prompt. `metadata` is an opaque bag for provider-specific
    extras (mem0's embedder fingerprint, a future native provider's source
    message id, etc.) — accessible if needed but never required.
    """

    id: str
    text: str
    key: str
    created_at: datetime | None = None
    metadata: dict[str, Any] = field(default_factory=dict)


@dataclass(frozen=True)
class MemorySearchHit:
    """A search result: the record plus the relevance score that surfaced it."""

    record: MemoryRecord
    score: float


# Default page size for `MemoryProvider.list`. Settings UIs use this as the
# initial page; callers wanting more pass `limit` explicitly.
DEFAULT_LIST_LIMIT = 50


class MemoryProvider(Protocol):
    """Async interface every memory backend must implement.

    `key` is an opaque namespacing string. Callers use it to scope memory
    by user (`user:<user_id>`) or by background agent (`agent:<agent_id>`);
    see `user_key` / `agent_key`.
    """

    async def add(self, messages: list[MessageParam], key: str) -> None:
        """Persist a conversation turn under `key`. Fire-and-forget."""
        ...

    async def search(
        self, query: str, key: str, limit: int = 5
    ) -> list[MemorySearchHit]:
        """Return up to `limit` memories under `key` ranked by relevance to `query`."""
        ...

    async def list(
        self,
        key: str,
        limit: int = DEFAULT_LIST_LIMIT,
        offset: int = 0,
    ) -> list[MemoryRecord]:
        """Return memories stored under `key`, newest first.

        `limit` and `offset` are best-effort: providers SHOULD honour them,
        but callers must tolerate fewer rows being returned (e.g. a small
        backend that fetches all rows and slices in memory).
        """
        ...

    async def delete(self, memory_id: str) -> bool:
        """Delete a single memory by id. Returns True on success."""
        ...

    async def delete_all(self, key: str) -> int:
        """Delete every memory under `key`. Returns total rows purged."""
        ...
