"""Memory management endpoints — session-scoped proxy over MemoryProvider.

The browser never talks to the memory backend directly: every call
goes through the AI service, which enforces that callers can only read
or delete their own memories. The user identity is taken from the
`x-user-id` header the web app attaches (mirrors the agents router).

Namespace keys:
- `user:<user_id>` for chat memory.
- `agent:<agent_id>` for agent memory (org agents and personal agents share
  the same key shape — the agent id is unique across both types).
"""

import logging

from anthropic.types import MessageParam
from fastapi import APIRouter, HTTPException, Path, Query, Request
from pydantic import BaseModel

from memory import (
    DEFAULT_LIST_LIMIT,
    MemoryProvider,
    MemoryRecord,
    agent_key,
    user_key,
)
from state import AppState

router = APIRouter(prefix="/memories", tags=["memory"])
logger = logging.getLogger(__name__)


def _require_user_id(request: Request) -> str:
    user_id = request.headers.get("x-user-id")
    if not user_id:
        raise HTTPException(status_code=401, detail="User ID required")
    return user_id


def _require_memory_provider(request: Request) -> MemoryProvider:
    app_state: AppState = request.app.state
    provider = app_state.memory_provider
    if provider is None:
        raise HTTPException(status_code=503, detail="Memory service not configured")
    return provider


def _record_to_response(record: MemoryRecord) -> dict:
    """Render a MemoryRecord for the public API.

    Older clients expect the mem0-shaped fields (id/memory/created_at).
    We keep that surface and surface the metadata bag flat alongside.
    """
    return {
        "id": record.id,
        "memory": record.text,
        "key": record.key,
        "created_at": record.created_at.isoformat() if record.created_at else None,
        **record.metadata,
    }


@router.get("")
async def list_memories(
    request: Request,
    limit: int = Query(DEFAULT_LIST_LIMIT, ge=1, le=500),
    offset: int = Query(0, ge=0),
):
    """Return memories stored for the caller, paginated."""
    user_id = _require_user_id(request)
    provider = _require_memory_provider(request)
    records = await provider.list(key=user_key(user_id), limit=limit, offset=offset)
    return {"memories": [_record_to_response(r) for r in records]}


@router.delete("")
async def delete_all_memories(request: Request):
    """Delete every memory stored for the caller."""
    user_id = _require_user_id(request)
    provider = _require_memory_provider(request)
    await provider.delete_all(key=user_key(user_id))
    return {"status": "deleted"}


def _require_admin(request: Request) -> None:
    user_id = request.headers.get("x-user-id")
    role = request.headers.get("x-user-role")
    if not user_id or role != "admin":
        raise HTTPException(status_code=403, detail="Admin access required")


@router.delete("/agent/{agent_id}")
async def delete_agent_memories(
    request: Request,
    agent_id: str = Path(..., description="Agent id whose memory namespace to purge"),
):
    """Purge every memory stored under `agent:<agent_id>`.

    Called by the web layer when an agent (org or personal) is deleted,
    and available to admins for manual cleanup.
    """
    _require_admin(request)
    provider = _require_memory_provider(request)
    namespace = agent_key(agent_id)
    await provider.delete_all(key=namespace)
    logger.info(f"Purged agent memory namespace: {namespace}")
    return {"status": "deleted", "namespace": namespace}


class SeedAgentMemoryRequest(BaseModel):
    name: str
    instructions: str
    schedule_type: str
    schedule_value: str


@router.post("/agent/{agent_id}/seed")
async def seed_agent_memory(
    request: Request,
    agent_id: str = Path(..., description="Agent id to seed memory for"),
    body: SeedAgentMemoryRequest = ...,
):
    """Replace agent memory with a seed record derived from its instructions.

    Called by the web layer on agent create/update. Deletes the existing
    namespace first so stale memories from a previous instructions version
    do not linger.
    """
    _require_admin(request)
    provider = _require_memory_provider(request)

    namespace = agent_key(agent_id)

    # Delete existing memories so stale facts don't persist after edits.
    await provider.delete_all(key=namespace)

    messages: list[MessageParam] = [
        MessageParam(role="user", content=f"Agent task: {body.instructions}"),
        MessageParam(
            role="assistant",
            content=(
                f"I am the '{body.name}' agent. "
                f"My task: {body.instructions}. "
                f"Schedule: {body.schedule_type} {body.schedule_value}."
            ),
        ),
    ]
    await provider.add(messages=messages, key=namespace)
    logger.info(f"Seeded agent memory namespace: {namespace}")
    return {"status": "seeded", "namespace": namespace}


@router.delete("/{memory_id}")
async def delete_memory(
    request: Request,
    memory_id: str = Path(..., description="Memory id to delete"),
):
    """Delete a single memory. Callers can only delete their own memories —
    verified by listing and matching the id against their set before
    issuing the delete to the provider.
    """
    user_id = _require_user_id(request)
    provider = _require_memory_provider(request)

    # Use a high limit so the ownership check is exhaustive; a user with
    # more than this many memories effectively bypasses the page check
    # but the upper bound is well above any realistic memory count.
    owned = await provider.list(key=user_key(user_id), limit=10_000)
    owned_ids = {r.id for r in owned}
    if memory_id not in owned_ids:
        # Do not leak whether the id exists under another user.
        raise HTTPException(status_code=404, detail="Memory not found")

    ok = await provider.delete(memory_id=memory_id)
    if not ok:
        raise HTTPException(status_code=502, detail="Memory service delete failed")
    return {"status": "deleted"}
