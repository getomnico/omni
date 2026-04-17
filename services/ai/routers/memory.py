"""Memory management endpoints — session-scoped proxy over mem0.

The browser never talks to the mem0 sidecar directly: every call goes
through the AI service, which enforces that callers can only read or
delete their own memories. The user identity is taken from the
`x-user-id` header the web app attaches (mirrors the agents router).
"""

import logging

from fastapi import APIRouter, HTTPException, Path, Request

from state import AppState

router = APIRouter(prefix="/memories", tags=["memory"])
logger = logging.getLogger(__name__)


def _require_user_id(request: Request) -> str:
    user_id = request.headers.get("x-user-id")
    if not user_id:
        raise HTTPException(status_code=401, detail="User ID required")
    return user_id


def _require_memory_client(request: Request):
    app_state: AppState = request.app.state
    client = getattr(app_state, "memory_client", None)
    if client is None:
        raise HTTPException(status_code=503, detail="Memory service not configured")
    return client


@router.get("")
async def list_memories(request: Request):
    """Return every memory stored for the caller."""
    user_id = _require_user_id(request)
    client = _require_memory_client(request)
    memories = await client.list(user_id=user_id)
    return {"memories": memories}


@router.delete("")
async def delete_all_memories(request: Request):
    """Delete every memory stored for the caller."""
    user_id = _require_user_id(request)
    client = _require_memory_client(request)
    ok = await client.delete_all(user_id=user_id)
    if not ok:
        raise HTTPException(status_code=502, detail="Memory service delete failed")
    return {"status": "deleted"}


@router.delete("/{memory_id}")
async def delete_memory(
    request: Request,
    memory_id: str = Path(..., description="mem0 memory id"),
):
    """Delete a single memory. Callers can only delete their own memories —
    verified by listing and matching the id against their set before issuing
    the delete to mem0.
    """
    user_id = _require_user_id(request)
    client = _require_memory_client(request)

    owned = await client.list(user_id=user_id)
    owned_ids = {m.get("id") for m in owned if isinstance(m, dict)}
    if memory_id not in owned_ids:
        # Do not leak whether the id exists under another user.
        raise HTTPException(status_code=404, detail="Memory not found")

    ok = await client.delete(memory_id=memory_id)
    if not ok:
        raise HTTPException(status_code=502, detail="Memory service delete failed")
    return {"status": "deleted"}
