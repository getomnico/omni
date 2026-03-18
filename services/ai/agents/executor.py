"""Non-interactive agent executor — mirrors the chat loop but without streaming/approval."""

import asyncio
import json
import logging
from datetime import datetime, timezone
from typing import cast

import httpx
from anthropic.types import (
    MessageParam,
    TextBlockParam,
    ToolUseBlockParam,
    ToolResultBlockParam,
)

from config import (
    AGENT_MAX_ITERATIONS,
    DEFAULT_MAX_TOKENS,
    DEFAULT_TEMPERATURE,
    DEFAULT_TOP_P,
    CONNECTOR_MANAGER_URL,
    SANDBOX_URL,
)
from db.documents import DocumentsRepository
from db.models import Source
from providers import LLMProvider
from prompts import build_agent_system_prompt
from services.compaction import ConversationCompactor
from state import AppState
from tools import (
    ToolRegistry,
    ToolContext,
    SearchToolHandler,
    ConnectorToolHandler,
    DocumentToolHandler,
)
from tools.connector_handler import ConnectorAction
from tools.email_handler import EmailToolHandler
from tools.sandbox_handler import SandboxToolHandler

from .models import Agent, AgentRun
from .repository import AgentRunRepository

logger = logging.getLogger(__name__)

MAX_RETRIES = 3


def _resolve_llm_provider(state: AppState, agent: Agent) -> LLMProvider:
    """Resolve which LLM provider to use for an agent."""
    models = state.models
    if not models:
        raise RuntimeError("No models configured")

    if agent.model_id and agent.model_id in models:
        return models[agent.model_id]

    if state.default_model_id and state.default_model_id in models:
        return models[state.default_model_id]

    return next(iter(models.values()))


async def _fetch_sources() -> list[Source] | None:
    """Fetch all sources from the connector manager."""
    if not CONNECTOR_MANAGER_URL:
        return None
    try:
        async with httpx.AsyncClient(timeout=10.0) as client:
            resp = await client.get(f"{CONNECTOR_MANAGER_URL.rstrip('/')}/sources")
            resp.raise_for_status()
            return [Source.from_row(s) for s in resp.json()]
    except Exception as e:
        logger.warning(f"Failed to fetch sources: {e}")
        return None


def _build_source_filter(agent: Agent) -> dict[str, list[str]] | None:
    """Build source_filter dict from agent.allowed_sources."""
    if not agent.allowed_sources:
        return None
    return {
        entry["source_id"]: entry.get("modes", ["read"])
        for entry in agent.allowed_sources
    }


async def _build_agent_registry(
    app_state: AppState,
    agent: Agent,
    sources: list[Source] | None,
) -> tuple[ToolRegistry, list[ConnectorAction] | None]:
    """Build a ToolRegistry configured for the agent's permissions."""
    registry = ToolRegistry()

    source_filter = _build_source_filter(agent) if agent.agent_type == "user" else None
    action_whitelist = agent.allowed_actions if agent.agent_type == "org" else None

    connector_actions: list[ConnectorAction] | None = None

    if CONNECTOR_MANAGER_URL:
        connector_handler = ConnectorToolHandler(
            connector_manager_url=CONNECTOR_MANAGER_URL,
            user_id=agent.user_id,
            redis_client=app_state.redis_client,
            prefetched_sources=sources,
            source_filter=source_filter,
            action_whitelist=action_whitelist,
        )
        await connector_handler._ensure_initialized()
        registry.register(connector_handler)

        if connector_handler._actions:
            connector_actions = list(connector_handler._actions.values())

    # Search tool — always registered
    search_operators = None
    if CONNECTOR_MANAGER_URL and connector_actions:
        search_operators = connector_handler.search_operators

    registry.register(
        SearchToolHandler(
            searcher_tool=app_state.searcher_tool,
            search_operators=search_operators,
        )
    )

    # Document handler
    content_storage = app_state.content_storage
    if content_storage or CONNECTOR_MANAGER_URL:
        registry.register(
            DocumentToolHandler(
                content_storage=content_storage,
                documents_repo=DocumentsRepository(),
                sandbox_url=SANDBOX_URL or None,
                connector_manager_url=CONNECTOR_MANAGER_URL or None,
            )
        )

    # Sandbox tools
    if SANDBOX_URL:
        registry.register(SandboxToolHandler(sandbox_url=SANDBOX_URL))

    # Email tool — only for org agents with send_email in allowed_actions
    if (
        agent.agent_type == "org"
        and action_whitelist
        and "send_email" in action_whitelist
    ):
        registry.register(EmailToolHandler())

    return registry, connector_actions


async def _run_agent_loop(
    agent: Agent,
    app_state: AppState,
    run: AgentRun,
    run_repo: AgentRunRepository,
    status_queue: asyncio.Queue | None,
) -> AgentRun:
    """Core agent loop. Separated from execute_agent to allow retries."""

    async def emit_status(message: str):
        if status_queue:
            await status_queue.put({"type": "status", "message": message})

    await emit_status("Initializing...")

    llm_provider = _resolve_llm_provider(app_state, agent)
    sources = await _fetch_sources()

    registry, connector_actions = await _build_agent_registry(app_state, agent, sources)
    all_tools = registry.get_all_tools()

    # Build system prompt
    active_sources = [s for s in (sources or []) if s.is_active and not s.is_deleted]
    system_prompt = build_agent_system_prompt(agent, active_sources, connector_actions)

    # Initialize conversation with a single trigger message
    conversation_messages: list[MessageParam] = [
        MessageParam(role="user", content="Execute your scheduled task now.")
    ]
    execution_log: list[MessageParam] = list(conversation_messages)

    # Org agents search all data (no user-scoping); personal agents are scoped to owner
    # Using run ID as chat_id — tool handlers use this to scope sandbox workspaces and cache keys
    context = ToolContext(
        chat_id=run.id,
        user_id=None if agent.agent_type == "org" else agent.user_id,
    )

    # Compaction support
    compactor = ConversationCompactor(
        llm_provider=llm_provider,
        redis_client=app_state.redis_client,
    )

    for iteration in range(AGENT_MAX_ITERATIONS):
        logger.info(f"Agent {agent.id} run {run.id}: iteration {iteration + 1}")

        # Check if compaction is needed
        if compactor.needs_compaction(conversation_messages, all_tools):
            logger.info(f"Compacting conversation for agent run {run.id}")
            # Using run ID as chat_id for compaction cache key
            conversation_messages = await compactor.compact_conversation(
                run.id, conversation_messages
            )

        # Call LLM (non-streaming — collect full response)
        content_blocks: list[TextBlockParam | ToolUseBlockParam] = []

        stream = llm_provider.stream_response(
            prompt="",
            messages=conversation_messages,
            tools=all_tools,
            max_tokens=DEFAULT_MAX_TOKENS,
            temperature=DEFAULT_TEMPERATURE,
            top_p=DEFAULT_TOP_P,
            system_prompt=system_prompt,
        )

        async for event in stream:
            if event.type == "content_block_start":
                if event.content_block.type == "text":
                    content_blocks.append(
                        TextBlockParam(type="text", text=event.content_block.text)
                    )
                elif event.content_block.type == "tool_use":
                    content_blocks.append(
                        ToolUseBlockParam(
                            type="tool_use",
                            id=event.content_block.id,
                            name=event.content_block.name,
                            input="",
                        )
                    )
            elif event.type == "content_block_delta":
                if event.delta.type == "text_delta":
                    if event.index < len(content_blocks):
                        text_block = cast(TextBlockParam, content_blocks[event.index])
                        text_block["text"] += event.delta.text
                elif event.delta.type == "input_json_delta":
                    if event.index < len(content_blocks):
                        tool_block = cast(
                            ToolUseBlockParam, content_blocks[event.index]
                        )
                        tool_block["input"] = (
                            cast(str, tool_block["input"]) + event.delta.partial_json
                        )
            elif event.type == "message_stop":
                break

        # Parse tool call inputs — on failure, send error back to LLM
        tool_calls = [b for b in content_blocks if b["type"] == "tool_use"]
        parse_errors: list[ToolResultBlockParam] = []
        for tool_call in tool_calls:
            raw_input = cast(str, tool_call["input"])
            try:
                tool_call["input"] = json.loads(raw_input)
            except json.JSONDecodeError as e:
                logger.warning(
                    f"Failed to parse tool call input for {tool_call['name']}: {e}"
                )
                tool_call["input"] = {}
                parse_errors.append(
                    ToolResultBlockParam(
                        type="tool_result",
                        tool_use_id=tool_call["id"],
                        content=[
                            {
                                "type": "text",
                                "text": f"Invalid JSON in tool input: {e}. Please retry with valid JSON.",
                            }
                        ],
                        is_error=True,
                    )
                )

        assistant_message = MessageParam(role="assistant", content=content_blocks)
        conversation_messages.append(assistant_message)
        execution_log.append(assistant_message)

        # If there were parse errors, feed them back to the LLM and continue the loop
        if parse_errors:
            error_message = MessageParam(role="user", content=parse_errors)
            conversation_messages.append(error_message)
            execution_log.append(error_message)
            continue

        # No tool calls — done
        if not tool_calls:
            logger.info(f"Agent {agent.id} run {run.id}: no tool calls, completing")
            break

        # Execute tool calls — no approval needed
        tool_results: list[ToolResultBlockParam] = []
        for tool_call in tool_calls:
            tool_name = tool_call["name"]
            await emit_status(f"Executing: {tool_name}")

            result = await registry.execute(tool_name, tool_call["input"], context)
            tool_results.append(
                ToolResultBlockParam(
                    type="tool_result",
                    tool_use_id=tool_call["id"],
                    content=result.content,
                    is_error=result.is_error,
                )
            )

        tool_result_message = MessageParam(role="user", content=tool_results)
        conversation_messages.append(tool_result_message)
        execution_log.append(tool_result_message)

    # Generate summary using one final LLM turn
    await emit_status("Generating summary...")
    summary_prompt_message = MessageParam(
        role="user",
        content=(
            "Provide a brief summary (2-3 sentences) of what you just did and the outcomes. "
            "Be factual and concise."
        ),
    )
    conversation_messages.append(summary_prompt_message)

    summary_blocks: list = []
    summary_stream = llm_provider.stream_response(
        prompt="",
        messages=conversation_messages,
        tools=[],
        max_tokens=500,
        temperature=0.3,
        system_prompt=system_prompt,
    )
    async for event in summary_stream:
        if event.type == "content_block_start" and event.content_block.type == "text":
            summary_blocks.append(event.content_block.text)
        elif event.type == "content_block_delta" and event.delta.type == "text_delta":
            summary_blocks.append(event.delta.text)
        elif event.type == "message_stop":
            break

    summary = "".join(summary_blocks).strip()

    completed_at = datetime.now(timezone.utc)
    run = await run_repo.update_run(
        run.id,
        status="completed",
        completed_at=completed_at,
        execution_log=execution_log,
        summary=summary,
    )

    if status_queue:
        await status_queue.put({"type": "completed", "summary": summary})

    logger.info(f"Agent {agent.id} run {run.id} completed successfully")
    return run


async def execute_agent(
    agent: Agent,
    app_state: AppState,
    status_queue: asyncio.Queue | None = None,
    run: AgentRun | None = None,
) -> AgentRun:
    """Execute a background agent run with retry support.

    Args:
        run: Optional pre-created AgentRun. If None, a new one is created.
    Retries up to MAX_RETRIES times on failure before giving up.
    """
    run_repo = AgentRunRepository()
    if run is None:
        run = await run_repo.create_run(agent.id)

    now = datetime.now(timezone.utc)
    run = await run_repo.update_run(run.id, status="running", started_at=now)

    last_error: Exception | None = None

    for attempt in range(1, MAX_RETRIES + 1):
        try:
            if attempt > 1:
                logger.info(
                    f"Agent {agent.id} run {run.id}: retry attempt {attempt}/{MAX_RETRIES}"
                )
            return await _run_agent_loop(agent, app_state, run, run_repo, status_queue)
        except Exception as e:
            last_error = e
            logger.error(
                f"Agent {agent.id} run {run.id} attempt {attempt} failed: {e}",
                exc_info=True,
            )
            if attempt < MAX_RETRIES:
                await asyncio.sleep(2**attempt)

    # All retries exhausted
    completed_at = datetime.now(timezone.utc)
    run = await run_repo.update_run(
        run.id,
        status="failed",
        completed_at=completed_at,
        execution_log=[],
        error_message=f"Failed after {MAX_RETRIES} attempts: {last_error}",
    )

    if status_queue:
        await status_queue.put({"type": "failed", "error": str(last_error)})

    return run
