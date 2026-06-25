"""Durable queue worker for agent_runs.

Lease contract:
- `agent_runs` rows in `pending` are durable work.
- Claiming sets `running`, a fresh ULID `claim_token`, heartbeat timestamps, and a
  lease expiry. The token is held only in memory by the active asyncio task.
- Heartbeat, completion, failure, and agent_run_logs WAL writes are fenced by
  `(run_id, claim_token)`, so stale tasks cannot finalize after lease recovery.
- `AGENT_MAX_CONCURRENT_RUNS` limits both local asyncio run tasks in this process
  and the cluster-wide count enforced inside the Postgres claim transaction.
- Agent conversation/action history is written incrementally to `agent_run_logs`:
  assistant tool-use messages are inserted before tool execution, and tool
  results are inserted immediately after each tool returns.
- Delivery is at-least-once. A crash can happen after an external side effect but
  before its result is logged; recovery records an interrupted tool result so the
  next LLM turn has durable context to reconcile.
"""

from __future__ import annotations

import asyncio
import logging
from datetime import timedelta

from config import (
    AGENT_MAX_CONCURRENT_RUNS,
    AGENT_RUN_BACKOFF_SECONDS,
    AGENT_RUN_CLAIM_POLL_INTERVAL_SECONDS,
    AGENT_RUN_HEARTBEAT_INTERVAL_SECONDS,
    AGENT_RUN_LEASE_SECONDS,
    AGENT_RUN_MAX_ATTEMPTS,
    AGENT_RUN_STALE_RECOVERY_INTERVAL_SECONDS,
)
from state import AppState

from .executor import execute_claimed_agent
from .models import AgentRunClaim, AgentRunRetryPolicy
from .repository import AgentRepository, AgentRunRepository

logger = logging.getLogger(__name__)


def _retry_policy() -> AgentRunRetryPolicy:
    return AgentRunRetryPolicy(
        max_attempts=AGENT_RUN_MAX_ATTEMPTS,
        backoff_delays=tuple(timedelta(seconds=s) for s in AGENT_RUN_BACKOFF_SECONDS),
    )


def _lease_duration() -> timedelta:
    return timedelta(seconds=AGENT_RUN_LEASE_SECONDS)


async def run_agent_queue_worker(app_state: AppState) -> None:
    """Run stale recovery and the durable claim loop forever."""
    run_repo = AgentRunRepository()
    retry_policy = _retry_policy()

    logger.info(
        "Agent queue worker started (max_concurrent=%s, lease=%ss, heartbeat=%ss)",
        AGENT_MAX_CONCURRENT_RUNS,
        AGENT_RUN_LEASE_SECONDS,
        AGENT_RUN_HEARTBEAT_INTERVAL_SECONDS,
    )

    await run_repo.recover_stale_runs()
    recovery_task = asyncio.create_task(_recovery_loop(run_repo), name="agent-run-recovery")

    active_tasks: set[asyncio.Task[None]] = set()
    try:
        while True:
            active_tasks = {task for task in active_tasks if not task.done()}

            while len(active_tasks) < AGENT_MAX_CONCURRENT_RUNS:
                claim = await run_repo.claim_next_run(
                    max_concurrent_runs=AGENT_MAX_CONCURRENT_RUNS,
                    lease_duration=_lease_duration(),
                    retry_policy=retry_policy,
                )
                if claim is None:
                    break

                task = asyncio.create_task(
                    _execute_claimed_run(app_state, claim, retry_policy),
                    name=f"agent-run-{claim.run.id}",
                )
                task.add_done_callback(_log_task_result)
                active_tasks.add(task)

            await asyncio.sleep(AGENT_RUN_CLAIM_POLL_INTERVAL_SECONDS)
    finally:
        recovery_task.cancel()
        for task in active_tasks:
            task.cancel()
        await asyncio.gather(recovery_task, *active_tasks, return_exceptions=True)


async def _recovery_loop(run_repo: AgentRunRepository) -> None:
    while True:
        try:
            await asyncio.sleep(AGENT_RUN_STALE_RECOVERY_INTERVAL_SECONDS)
            await run_repo.recover_stale_runs()
        except asyncio.CancelledError:
            raise
        except Exception as e:
            logger.error("Agent stale-run recovery failed: %s", e, exc_info=True)


def _log_task_result(task: asyncio.Task[None]) -> None:
    if task.cancelled():
        return
    exc = task.exception()
    if exc is not None:
        logger.error(
            "Agent run task failed unexpectedly: %s",
            exc,
            exc_info=(type(exc), exc, exc.__traceback__),
        )


async def _execute_claimed_run(
    app_state: AppState,
    claim: AgentRunClaim,
    retry_policy: AgentRunRetryPolicy,
) -> None:
    run_repo = AgentRunRepository()
    agent_repo = AgentRepository()
    current_task = asyncio.current_task()
    heartbeat_task = asyncio.create_task(
        _heartbeat_loop(run_repo, claim, current_task),
        name=f"agent-run-heartbeat-{claim.run.id}",
    )

    try:
        agent = await agent_repo.get_agent(claim.run.agent_id)
        if agent is None or agent.is_deleted or not agent.is_enabled:
            failed = await run_repo.fail_run(
                claim.run.id,
                claim.claim_token,
                "Agent is disabled, deleted, or no longer exists",
                AgentRunRetryPolicy(max_attempts=0, backoff_delays=()),
            )
            if failed is None:
                logger.warning("Lost claim while failing unavailable agent run %s", claim.run.id)
            return

        result = await execute_claimed_agent(
            agent,
            app_state,
            claim.run,
            claim.claim_token,
            run_repo,
        )
        completed = await run_repo.complete_run(
            claim.run.id,
            claim.claim_token,
            result.summary,
        )
        if completed is None:
            logger.warning("Lost claim before completing agent run %s", claim.run.id)
    except asyncio.CancelledError:
        logger.warning("Agent run %s task cancelled", claim.run.id)
        raise
    except Exception as e:
        logger.error("Agent run %s failed: %s", claim.run.id, e, exc_info=True)
        failed = await run_repo.fail_run(
            claim.run.id,
            claim.claim_token,
            str(e),
            retry_policy,
        )
        if failed is None:
            logger.warning("Lost claim before failing agent run %s", claim.run.id)
    finally:
        heartbeat_task.cancel()
        await asyncio.gather(heartbeat_task, return_exceptions=True)


async def _heartbeat_loop(
    run_repo: AgentRunRepository,
    claim: AgentRunClaim,
    run_task: asyncio.Task[None] | None,
) -> None:
    while True:
        try:
            await asyncio.sleep(AGENT_RUN_HEARTBEAT_INTERVAL_SECONDS)
            ok = await run_repo.heartbeat_run(
                claim.run.id,
                claim.claim_token,
                _lease_duration(),
            )
            if not ok:
                logger.warning("Agent run %s lost its claim; cancelling task", claim.run.id)
                if run_task is not None:
                    run_task.cancel()
                return
        except asyncio.CancelledError:
            raise
        except Exception as e:
            logger.error("Heartbeat for agent run %s failed: %s", claim.run.id, e, exc_info=True)
