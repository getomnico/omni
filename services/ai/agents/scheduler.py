"""Background materializer for scheduled agent runs."""

import asyncio
import logging
from datetime import UTC, datetime

from config import AGENT_MAX_CONCURRENT_RUNS, AGENT_SCHEDULER_POLL_INTERVAL
from state import AppState

from .models import AgentRunAlreadyActive, AgentRunTriggerType
from .repository import AgentRepository, AgentRunRepository

logger = logging.getLogger(__name__)


async def materialize_due_agent_runs(
    agent_repo: AgentRepository,
    run_repo: AgentRunRepository,
    now: datetime,
) -> int:
    """Insert pending scheduled runs for due agents and return created count."""
    due_agents = await agent_repo.find_due_agents(now)

    if due_agents:
        logger.info("Found %d due agent(s)", len(due_agents))

    created_count = 0
    for agent in due_agents:
        result = await run_repo.create_run(
            agent.id, trigger_type=AgentRunTriggerType.SCHEDULED
        )
        if isinstance(result, AgentRunAlreadyActive):
            logger.debug(
                "Skipped scheduled run for agent %s because run %s is %s",
                agent.id,
                result.run.id,
                result.run.status.value,
            )
        else:
            created_count += 1
            logger.info(
                "Materialized scheduled run %s for agent %s",
                result.id,
                agent.id,
            )
    return created_count


async def run_agent_schedule_materializer(app_state: AppState) -> None:
    """Poll due agents and insert pending scheduled runs only.

    Execution is handled by the durable queue worker; the scheduler never starts
    agent execution directly.
    """
    logger.info(
        "Agent schedule materializer started (poll_interval=%ss, max_concurrent=%s)",
        AGENT_SCHEDULER_POLL_INTERVAL,
        AGENT_MAX_CONCURRENT_RUNS,
    )

    agent_repo = AgentRepository()
    run_repo = AgentRunRepository()

    while True:
        try:
            now = datetime.now(UTC)
            await materialize_due_agent_runs(agent_repo, run_repo, now)

        except Exception as e:
            logger.error("Agent schedule materializer tick failed: %s", e, exc_info=True)

        await asyncio.sleep(AGENT_SCHEDULER_POLL_INTERVAL)


async def run_agent_scheduler(app_state: AppState) -> None:
    """Backward-compatible entrypoint for the schedule materializer."""
    await run_agent_schedule_materializer(app_state)
