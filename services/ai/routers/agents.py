"""Agent API endpoints — trigger and live status streaming.

Run history (list/detail) is read directly from the DB by omni-web.
"""

import asyncio
import json
import logging

from fastapi import APIRouter, HTTPException, Path, Request
from fastapi.responses import StreamingResponse

from agents.models import Agent
from agents.repository import AgentRepository, AgentRunRepository
from agents.executor import execute_agent
from db import UsersRepository
from state import AppState

router = APIRouter(prefix="/agents", tags=["agents"])
logger = logging.getLogger(__name__)


async def _get_agent_with_auth(request: Request, agent_id: str, user_id: str) -> Agent:
    """Fetch agent and verify ownership/admin access."""
    agent_repo = AgentRepository()
    agent = await agent_repo.get_agent(agent_id)
    if not agent:
        raise HTTPException(status_code=404, detail="Agent not found")

    # User agents: owner only. Org agents: admin only.
    if agent.agent_type == "org":
        users_repo = UsersRepository()
        user = await users_repo.find_by_id(user_id)
        if not user or user.role != "admin":
            raise HTTPException(status_code=403, detail="Admin access required")
    elif agent.user_id != user_id:
        raise HTTPException(status_code=403, detail="Access denied")

    return agent


@router.post("/{agent_id}/trigger")
async def trigger_agent(
    request: Request,
    agent_id: str = Path(...),
):
    """Manually trigger an agent run."""
    user_id = request.headers.get("x-user-id")
    if not user_id:
        raise HTTPException(status_code=401, detail="User ID required")

    agent = await _get_agent_with_auth(request, agent_id, user_id)

    app_state: AppState = request.app.state

    # Create the run upfront so we can return its ID immediately
    run_repo = AgentRunRepository()
    run = await run_repo.create_run(agent_id)

    status_queue: asyncio.Queue = asyncio.Queue()

    async def _run():
        try:
            await execute_agent(agent, app_state, status_queue=status_queue, run=run)
        except Exception as e:
            logger.error(f"Triggered run for agent {agent_id} failed: {e}")

    asyncio.create_task(_run())

    return {"status": "started", "run_id": run.id}


@router.get("/{agent_id}/runs/{run_id}/stream")
async def stream_run_status(
    request: Request,
    agent_id: str = Path(...),
    run_id: str = Path(...),
):
    """SSE stream of live status events for an in-progress run."""
    user_id = request.headers.get("x-user-id")
    if not user_id:
        raise HTTPException(status_code=401, detail="User ID required")

    await _get_agent_with_auth(request, agent_id, user_id)

    app_state: AppState = request.app.state
    queues = getattr(app_state, "agent_run_queues", {})
    queue = queues.get(run_id)

    async def event_generator():
        if not queue:
            yield f"event: error\ndata: No active stream for this run\n\n"
            return

        while True:
            if await request.is_disconnected():
                break
            try:
                event = await asyncio.wait_for(queue.get(), timeout=30.0)
                yield f"event: {event['type']}\ndata: {json.dumps(event)}\n\n"

                if event["type"] in ("completed", "failed"):
                    break
            except asyncio.TimeoutError:
                yield f"event: ping\ndata: {{}}\n\n"

    return StreamingResponse(
        event_generator(),
        media_type="text/event-stream",
        headers={"Cache-Control": "no-cache", "Connection": "keep-alive"},
    )
