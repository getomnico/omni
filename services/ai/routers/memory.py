"""Memory management endpoints — session-scoped proxy over mem0.

The browser never talks to the mem0 sidecar directly: every call goes
through the AI service, which enforces that callers can only read or
delete their own memories. The user identity is taken from the
`x-user-id` header the web app attaches (mirrors the agents router).
"""

import logging

from fastapi import APIRouter, HTTPException, Path, Query, Request
from pydantic import BaseModel

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


def _require_admin(request: Request) -> None:
    user_id = request.headers.get("x-user-id")
    role = request.headers.get("x-user-role")
    if not user_id or role != "admin":
        raise HTTPException(status_code=403, detail="Admin access required")


@router.delete("/org-agent/{agent_id}")
async def delete_org_agent_memories(
    request: Request,
    agent_id: str = Path(..., description="Agent id whose memory namespace to purge"),
):
    """Purge every memory stored under `org_agent:<agent_id>`.

    Used by the web layer when an org agent is deleted, and available
    to admins for manual cleanup of policy-sensitive memories.
    """
    _require_admin(request)
    client = _require_memory_client(request)
    namespace = f"org_agent:{agent_id}"
    ok = await client.delete_all(user_id=namespace)
    if not ok:
        raise HTTPException(status_code=502, detail="Memory service delete failed")
    logger.info(f"Purged org-agent memory namespace: {namespace}")
    return {"status": "deleted", "namespace": namespace}


@router.delete("/user-agent/{agent_id}")
async def delete_user_agent_memories(
    request: Request,
    agent_id: str = Path(..., description="Agent id whose memory namespace to purge"),
    owner_user_id: str = Query(..., description="User id of the agent owner"),
):
    """Purge every memory stored under `user:<owner_user_id>:agent:<agent_id>`.

    Used by the web layer when a personal agent is deleted.
    """
    _require_admin(request)
    client = _require_memory_client(request)
    namespace = f"user:{owner_user_id}:agent:{agent_id}"
    ok = await client.delete_all(user_id=namespace)
    if not ok:
        raise HTTPException(status_code=502, detail="Memory service delete failed")
    logger.info(f"Purged user-agent memory namespace: {namespace}")
    return {"status": "deleted", "namespace": namespace}


class SeedAgentMemoryRequest(BaseModel):
    owner_user_id: str | None = None  # None for org agents
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
    client = _require_memory_client(request)

    namespace = (
        f"org_agent:{agent_id}"
        if body.owner_user_id is None
        else f"user:{body.owner_user_id}:agent:{agent_id}"
    )

    # Delete existing memories so stale facts don't persist after edits.
    await client.delete_all(user_id=namespace)

    messages = [
        {"role": "user", "content": f"Agent task: {body.instructions}"},
        {
            "role": "assistant",
            "content": (
                f"I am the '{body.name}' agent. "
                f"My task: {body.instructions}. "
                f"Schedule: {body.schedule_type} {body.schedule_value}."
            ),
        },
    ]
    await client.add(messages=messages, user_id=namespace)
    logger.info(f"Seeded agent memory namespace: {namespace}")
    return {"status": "seeded", "namespace": namespace}


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
