"""Non-interactive agent executor — mirrors the chat loop but without streaming/approval."""

import asyncio
import json
import logging
from dataclasses import dataclass
from datetime import timedelta
from pathlib import Path
from typing import cast

import httpx
from anthropic.types import (
    BashCodeExecutionToolResultBlockParam,
    CodeExecutionToolResultBlockParam,
    ContainerUploadBlockParam,
    ContentBlock,
    DocumentBlockParam,
    ImageBlockParam,
    MessageParam,
    RedactedThinkingBlockParam,
    SearchResultBlockParam,
    ServerToolUseBlockParam,
    TextBlockParam,
    TextEditorCodeExecutionToolResultBlockParam,
    ThinkingBlockParam,
    ToolParam,
    ToolResultBlockParam,
    ToolSearchToolResultBlockParam,
    ToolUseBlockParam,
    WebFetchToolResultBlockParam,
    WebSearchToolResultBlockParam,
)

from config import (
    AGENT_MAX_CONCURRENT_RUNS,
    AGENT_MAX_ITERATIONS,
    AGENT_RUN_BACKOFF_SECONDS,
    AGENT_RUN_LEASE_SECONDS,
    AGENT_RUN_MAX_ATTEMPTS,
    CONNECTOR_MANAGER_URL,
    DEFAULT_MAX_TOKENS,
    DEFAULT_TEMPERATURE,
    DEFAULT_TOP_P,
    SANDBOX_URL,
)
from db import CompactionsRepository, SkillsRepository
from db.configuration import ConfigurationRepository
from db.documents import DocumentsRepository
from db.models import Source, UserConfiguration
from db.usage import UsageRepository
from db.users import UsersRepository
from memory import MemoryMode, agent_key, resolve_memory_mode
from prompts import build_agent_system_prompt
from providers import LLMProvider, ProviderError
from services.compaction import ConversationCompactor
from services.usage import UsageContext, UsagePurpose, UsageTracker, track_usage
from state import AppState
from tools import (
    DocumentToolHandler,
    PeopleSearchHandler,
    SearchToolHandler,
    ToolContext,
    ToolHandler,
    ToolRegistry,
    WebToolHandler,
)
from tools.connector_handler import (
    ConnectorToolHandler,
    SourceFilter,
    ToolsetSummary,
    sources_from_sync_overview_response,
)
from tools.email_handler import EmailToolHandler
from tools.mcp_capability_handler import McpCapabilityHandler
from tools.meta_handler import MetaToolHandler
from tools.sandbox_handler import SandboxToolHandler
from tools.search_handler import fetch_operator_values
from tools.skill_handler import SkillHandler
from tools.turn_builder import build_turn_tools

from .models import (
    Agent,
    AgentExecutionResult,
    AgentRun,
    AgentRunAlreadyActive,
    AgentRunLogMessage,
    AgentRunRetryPolicy,
    AgentRunTriggerType,
)
from .repository import AgentRunRepository

logger = logging.getLogger(__name__)

AgentContentBlock = (
    TextBlockParam
    | ImageBlockParam
    | DocumentBlockParam
    | SearchResultBlockParam
    | ThinkingBlockParam
    | RedactedThinkingBlockParam
    | ToolUseBlockParam
    | ToolResultBlockParam
    | ServerToolUseBlockParam
    | WebSearchToolResultBlockParam
    | WebFetchToolResultBlockParam
    | CodeExecutionToolResultBlockParam
    | BashCodeExecutionToolResultBlockParam
    | TextEditorCodeExecutionToolResultBlockParam
    | ToolSearchToolResultBlockParam
    | ContainerUploadBlockParam
    | ContentBlock
)


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
    try:
        async with httpx.AsyncClient(timeout=10.0) as client:
            resp = await client.get(f"{CONNECTOR_MANAGER_URL.rstrip('/')}/sources")
            resp.raise_for_status()
            return sources_from_sync_overview_response(resp.json())
    except Exception as e:
        logger.warning(f"Failed to fetch sources: {e}")
        return None


def _build_source_filter(agent: Agent) -> SourceFilter | None:
    """Build source_filter dict from agent.allowed_sources."""
    if not agent.allowed_sources:
        return None
    return {
        entry["source_id"]: entry.get("modes", ["read"])
        for entry in agent.allowed_sources
    }


@dataclass
class AgentRegistry:
    registry: ToolRegistry
    always_on_handlers: list[ToolHandler]
    connector_handlers: list[ConnectorToolHandler]
    toolsets: list[ToolsetSummary]

    def build_turn_tools(self, loaded_toolsets: set[str]) -> list[ToolParam]:
        connector_handler = (
            self.connector_handlers[0] if self.connector_handlers else None
        )
        return build_turn_tools(
            self.always_on_handlers, connector_handler, loaded_toolsets
        )


async def _build_agent_registry(
    app_state: AppState,
    agent: Agent,
    sources: list[Source] | None,
    loaded_toolsets: set[str],
) -> AgentRegistry:
    """Build a ToolRegistry configured for the agent's permissions.

    Connector tools are dispatched through the registry but exposed lazily via
    `MetaToolHandler` (issue #203). The agent's `source_filter` and
    `action_whitelist` already constrain which actions are visible.
    """
    registry = ToolRegistry()
    always_on_handlers: list[ToolHandler] = []

    source_filter = _build_source_filter(agent) if agent.agent_type == "user" else None
    action_whitelist = agent.allowed_actions if agent.agent_type == "org" else None

    # Org agents are admin-controlled; user agents take their owner's role.
    if agent.agent_type == "org":
        is_admin = True
    else:
        owner = await UsersRepository().find_by_id(agent.user_id)
        is_admin = bool(owner and owner.role == "admin")

    connector_handler: ConnectorToolHandler | None = None
    connector_handlers: list[ConnectorToolHandler] = []
    toolsets: list[ToolsetSummary] = []

    connector_handler = ConnectorToolHandler(
        connector_manager_url=CONNECTOR_MANAGER_URL,
        user_id=agent.user_id,
        redis_client=app_state.redis_client,
        prefetched_sources=sources,
        source_filter=source_filter,
        action_whitelist=action_whitelist,
        documents_repo=DocumentsRepository(),
        sandbox_url=SANDBOX_URL,
        is_admin=is_admin,
    )
    await connector_handler._ensure_initialized()
    registry.register(connector_handler)
    connector_handlers.append(connector_handler)

    if connector_handler.actions:
        toolsets = connector_handler.list_toolsets()

    # Meta-tools: register only when there's something for the agent to load.
    if connector_handler is not None and toolsets:
        meta_handler = MetaToolHandler(
            connector_handler=connector_handler,
            loaded=loaded_toolsets,
            on_load=_noop_on_load,
            searcher_client=app_state.searcher_tool.client,
        )
        await meta_handler.publish_tool_capabilities()
        registry.register(meta_handler)
        always_on_handlers.append(meta_handler)

    mcp_handler = McpCapabilityHandler(
        connector_manager_url=CONNECTOR_MANAGER_URL,
        searcher_client=app_state.searcher_tool.client,
        prefetched_sources=sources,
        source_filter=source_filter,
    )
    await mcp_handler.refresh()
    if mcp_handler.has_capabilities():
        await mcp_handler.publish_capabilities()
        registry.register(mcp_handler)
        always_on_handlers.append(mcp_handler)

    # Search tool — always registered
    search_operators = None
    if connector_handler is not None and toolsets:
        search_operators = connector_handler.search_operators

    active_sources = [s for s in (sources or []) if s.is_active and not s.is_deleted]
    connected_source_types = list({s.source_type for s in active_sources})
    operator_values: dict[str, list[str]] = {}
    if search_operators:
        operator_values = await fetch_operator_values(
            app_state.searcher_tool.client,
            search_operators,
            redis_client=app_state.redis_client,
        )

    search_handler = SearchToolHandler(
        searcher_tool=app_state.searcher_tool,
        search_operators=search_operators,
        connected_source_types=connected_source_types,
        operator_values=operator_values,
    )
    registry.register(search_handler)
    always_on_handlers.append(search_handler)

    if app_state.web_search_provider is not None:
        web_handler = WebToolHandler(
            search_provider=app_state.web_search_provider,
            fetch_provider=app_state.web_fetch_provider,
        )
        registry.register(web_handler)
        always_on_handlers.append(web_handler)

    people_handler = PeopleSearchHandler(searcher_tool=app_state.searcher_tool)
    registry.register(people_handler)
    always_on_handlers.append(people_handler)

    content_storage = app_state.content_storage
    document_handler = DocumentToolHandler(
        content_storage=content_storage,
        documents_repo=DocumentsRepository(),
        sandbox_url=SANDBOX_URL,
        connector_manager_url=CONNECTOR_MANAGER_URL,
    )
    registry.register(document_handler)
    always_on_handlers.append(document_handler)

    if SANDBOX_URL:
        sandbox_handler = SandboxToolHandler(sandbox_url=SANDBOX_URL)
        registry.register(sandbox_handler)
        always_on_handlers.append(sandbox_handler)

    skills_dir = Path(__file__).resolve().parent.parent / "skills"
    skill_handler = SkillHandler(
        skills_dir=skills_dir,
        searcher_client=app_state.searcher_tool.client,
        connector_manager_url=CONNECTOR_MANAGER_URL,
        skills_repository=SkillsRepository(),
        skill_user_id=agent.user_id,
    )
    await skill_handler.refresh_library_skills()
    await skill_handler.refresh_connector_skills()
    if skill_handler.has_skills():
        await skill_handler.publish_skill_capabilities()
        registry.register(skill_handler)
        always_on_handlers.append(skill_handler)

    # Email tool — only for org agents with send_email in allowed_actions
    if (
        agent.agent_type == "org"
        and action_whitelist
        and "send_email" in action_whitelist
    ):
        email_handler = EmailToolHandler()
        registry.register(email_handler)
        always_on_handlers.append(email_handler)

    return AgentRegistry(
        registry=registry,
        always_on_handlers=always_on_handlers,
        connector_handlers=connector_handlers,
        toolsets=toolsets,
    )


async def _noop_on_load(_: set[str]) -> None:
    """Agent runs are one-shot — loaded tools are not persisted across runs."""
    return None


def _content_blocks(message: MessageParam) -> list[AgentContentBlock]:
    content = message.get("content")
    return list(content) if isinstance(content, list) else []


def _tool_use_id(block: AgentContentBlock) -> str | None:
    if block.get("type") == "tool_use":
        tool_id = block.get("id")
        return str(tool_id) if tool_id is not None else None
    return None


def _tool_result_id(block: AgentContentBlock) -> str | None:
    if block.get("type") == "tool_result":
        tool_id = block.get("tool_use_id")
        return str(tool_id) if tool_id is not None else None
    return None


def _find_unanswered_tool_calls(
    messages: list[AgentRunLogMessage],
) -> list[ToolUseBlockParam]:
    pending: dict[str, ToolUseBlockParam] = {}
    for message in messages:
        for block in _content_blocks(message):
            tool_use_id = _tool_use_id(block)
            if tool_use_id is not None:
                pending[tool_use_id] = cast(ToolUseBlockParam, block)
                continue
            tool_result_id = _tool_result_id(block)
            if tool_result_id is not None:
                pending.pop(tool_result_id, None)
    return list(pending.values())


def _synthetic_interrupted_results(
    tool_calls: list[ToolUseBlockParam],
) -> MessageParam | None:
    if not tool_calls:
        return None
    results: list[ToolResultBlockParam] = []
    for tool_call in tool_calls:
        results.append(
            ToolResultBlockParam(
                type="tool_result",
                tool_use_id=tool_call["id"],
                content=[
                    {
                        "type": "text",
                        "text": (
                            "A previous attempt was interrupted after this tool call was "
                            "requested, before a tool result was durably recorded. Inspect "
                            "the prior context and retry or reconcile as appropriate."
                        ),
                    }
                ],
                is_error=True,
            )
        )
    return MessageParam(role="user", content=results)


def _is_tool_result_only_user_message(message: AgentRunLogMessage) -> bool:
    if message.get("role") != "user":
        return False
    blocks = _content_blocks(message)
    return bool(blocks) and all(_tool_result_id(block) is not None for block in blocks)


def _conversation_from_log_messages(
    messages: list[AgentRunLogMessage],
) -> list[MessageParam]:
    """Build provider conversation, coalescing adjacent persisted tool-result rows."""
    conversation: list[MessageParam] = []
    pending_tool_results: list[ToolResultBlockParam] = []

    def flush_tool_results() -> None:
        nonlocal pending_tool_results
        if pending_tool_results:
            conversation.append(MessageParam(role="user", content=pending_tool_results))
            pending_tool_results = []

    for message in messages:
        if _is_tool_result_only_user_message(message):
            pending_tool_results.extend(
                cast(list[ToolResultBlockParam], _content_blocks(message))
            )
        else:
            flush_tool_results()
            conversation.append(cast(MessageParam, message))
    flush_tool_results()
    return conversation


async def _load_or_initialize_conversation(
    run: AgentRun,
    run_repo: AgentRunRepository,
    claim_token: str,
) -> list[AgentRunLogMessage]:
    logs = await run_repo.list_run_logs(run.id)
    messages = [log.message for log in logs]
    if not messages:
        initial = MessageParam(role="user", content="Execute your scheduled task now.")
        await run_repo.append_run_log_messages(run.id, claim_token, [initial])
        messages.append(initial)
        return messages

    interrupted = _synthetic_interrupted_results(_find_unanswered_tool_calls(messages))
    if interrupted is not None:
        await run_repo.append_run_log_messages(run.id, claim_token, [interrupted])
        messages.append(interrupted)
    return messages


async def _append_log_message(
    run: AgentRun,
    run_repo: AgentRunRepository,
    claim_token: str,
    messages: list[AgentRunLogMessage],
    message: MessageParam,
) -> None:
    rows = await run_repo.append_run_log_messages(run.id, claim_token, [message])
    if not rows:
        raise RuntimeError("Lost agent run claim while appending conversation log")
    messages.append(message)


async def _run_agent_loop(
    agent: Agent,
    app_state: AppState,
    run: AgentRun,
    run_repo: AgentRunRepository,
    claim_token: str,
) -> AgentExecutionResult:
    """Core agent loop. Queue lifecycle finalization is handled by the worker."""

    logger.info("Agent %s run %s: initializing", agent.id, run.id)

    llm_provider = _resolve_llm_provider(app_state, agent)
    sources = await _fetch_sources()

    # Each agent run starts with no connector tools loaded — discovery is per-run.
    loaded_toolsets: set[str] = set()

    agent_registry = await _build_agent_registry(
        app_state, agent, sources, loaded_toolsets
    )
    registry = agent_registry.registry
    toolsets = agent_registry.toolsets

    # Org agents search all data (no user-scoping); personal agents are scoped to owner
    # Using run ID as chat_id — tool handlers use this to scope sandbox workspaces and cache keys
    is_org_agent = agent.agent_type == "org"
    agent_user_email: str | None = None
    agent_user_name: str | None = None
    agent_user_configuration: UserConfiguration | None = None
    agent_user = None
    if agent.user_id:
        users_repo = UsersRepository()
        agent_user = await users_repo.find_by_id(agent.user_id)
        if agent_user:
            agent_user_email = agent_user.email
            agent_user_name = agent_user.full_name
            agent_user_configuration = agent_user.configuration

    # Memory: resolve effective mode and fetch prior-run memories. Both
    # personal and org agents share the `agent:<id>` namespace; the
    # difference is only in how the effective mode is computed.
    memory_provider = app_state.memory_provider
    memory_namespace: str | None = None
    effective_mode = MemoryMode.OFF
    memories: list[str] = []
    if memory_provider is not None:
        config_repo = ConfigurationRepository()
        org_default = (await config_repo.get_global_configuration()).memory_mode_default
        if is_org_agent:
            effective_mode = org_default
        elif agent_user_configuration is not None:
            effective_mode = resolve_memory_mode(
                agent_user_configuration.memory_mode, org_default
            )
        memory_namespace = agent_key(agent.id)

        if effective_mode == MemoryMode.FULL and agent.instructions:
            hits = await memory_provider.search(
                query=agent.instructions, key=memory_namespace, limit=5
            )
            memories = [h.record.text for h in hits if h.record.text]

    # Build system prompt
    active_sources = [s for s in (sources or []) if s.is_active and not s.is_deleted]
    system_prompt = build_agent_system_prompt(
        agent,
        active_sources,
        toolsets=toolsets,
        loaded_source_ids=set(),
        user_name=agent_user_name if not is_org_agent else None,
        user_email=agent_user_email if not is_org_agent else None,
        memories=memories if memories else None,
        user_configuration=agent_user_configuration,
        include_web_search=app_state.web_search_provider is not None,
        include_fetch_web_page=app_state.web_fetch_provider is not None,
    )

    # Load durable conversation/action WAL, or initialize it before the first LLM call.
    log_messages = await _load_or_initialize_conversation(run, run_repo, claim_token)
    conversation_messages = _conversation_from_log_messages(log_messages)

    context = ToolContext(
        chat_id=run.id,
        user_id=None if is_org_agent else agent.user_id,
        user_email=agent_user_email,
        user_configuration=agent_user_configuration,
        skip_permission_check=is_org_agent,
    )

    # Compaction support — use secondary model for summarization when available
    secondary_provider = llm_provider
    if (
        app_state.secondary_model_id
        and app_state.secondary_model_id in app_state.models
    ):
        secondary_provider = app_state.models[app_state.secondary_model_id]

    def _on_compaction_usage(usage):
        track_usage(
            UsageRepository(),
            UsageContext(
                user_id=agent.user_id if not is_org_agent else None,
                model_id=secondary_provider.model_record_id,
                model_name=secondary_provider.model_name,
                provider_type=secondary_provider.provider_type,
                purpose=UsagePurpose.COMPACTION,
                agent_run_id=run.id,
            ),
            input_tokens=usage.input_tokens,
            output_tokens=usage.output_tokens,
            cache_read_tokens=usage.cache_read_tokens,
            cache_creation_tokens=usage.cache_creation_tokens,
        )

    compactor = ConversationCompactor(
        llm_provider=secondary_provider,
        on_usage=_on_compaction_usage,
    )
    compactions_repo = CompactionsRepository()
    summarizer_context = await secondary_provider.get_context_window_tokens()
    latest_compaction = None

    for iteration in range(AGENT_MAX_ITERATIONS):
        logger.info(f"Agent {agent.id} run {run.id}: iteration {iteration + 1}")

        # Per-turn tool list — picks up any connector tools the LLM loaded in the
        # previous iteration via load_tool / load_tool_set.
        turn_tools = agent_registry.build_turn_tools(loaded_toolsets)

        log_rows = await run_repo.list_run_logs(run.id)
        prepared = await compactor.prepare_agent_conversation(
            run_id=run.id,
            log_rows=log_rows,
            compactions_repo=compactions_repo,
            target_provider=llm_provider,
            tools=turn_tools,
            system_prompt=system_prompt,
            max_output_tokens=DEFAULT_MAX_TOKENS,
            coalesce_messages=lambda rows: _conversation_from_log_messages(
                [row.message for row in rows]
            ),
        )
        conversation_messages = prepared.messages
        latest_compaction = prepared.latest_compaction
        summarizer_context = prepared.summarizer_context
        logger.info(
            "Agent %s run %s context windows: model=%s (%s), summarizer=%s (%s)",
            agent.id,
            run.id,
            prepared.model_context.tokens,
            prepared.model_context.source,
            summarizer_context.tokens,
            summarizer_context.source,
        )

        # Call LLM (non-streaming — collect full response). If the provider says
        # the request exceeded its context window before any content is produced,
        # force one extra compaction pass and retry once.
        content_blocks: list[TextBlockParam | ToolUseBlockParam] = []
        tracker: UsageTracker | None = None
        for llm_attempt in range(2):
            content_blocks = []
            usage_repo = UsageRepository()
            tracker = UsageTracker(
                usage_repo,
                UsageContext(
                    user_id=agent.user_id if not is_org_agent else None,
                    model_id=llm_provider.model_record_id,
                    model_name=llm_provider.model_name,
                    provider_type=llm_provider.provider_type,
                    purpose=UsagePurpose.AGENT_RUN,
                    agent_run_id=run.id,
                ),
            )

            raw_stream = llm_provider.stream_response(
                prompt="",
                messages=conversation_messages,
                tools=turn_tools,
                max_tokens=DEFAULT_MAX_TOKENS,
                temperature=DEFAULT_TEMPERATURE,
                top_p=DEFAULT_TOP_P,
                system_prompt=system_prompt,
            )

            try:
                async for event in tracker.wrap_stream(raw_stream):
                    if event.type == "content_block_start":
                        if event.content_block.type == "text":
                            content_blocks.append(
                                TextBlockParam(
                                    type="text", text=event.content_block.text
                                )
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
                                text_block = cast(
                                    TextBlockParam, content_blocks[event.index]
                                )
                                text_block["text"] += event.delta.text
                        elif event.delta.type == "input_json_delta" and event.index < len(
                            content_blocks
                        ):
                            tool_block = cast(
                                ToolUseBlockParam, content_blocks[event.index]
                            )
                            tool_block["input"] = (
                                cast(str, tool_block["input"]) + event.delta.partial_json
                            )
                    elif event.type == "message_stop":
                        break
                break
            except ProviderError as e:
                if e.is_context_overflow and llm_attempt == 0 and not content_blocks:
                    logger.warning(
                        "Agent run %s hit provider context limit; retrying once after forced compaction",
                        run.id,
                    )
                    conversation_messages = await compactor.compact_conversation(
                        run.id,
                        conversation_messages,
                        previous_summary=(
                            latest_compaction.summary if latest_compaction else None
                        ),
                        summarizer_context_window_tokens=summarizer_context.tokens,
                    )
                    continue
                raise

        if tracker is not None:
            tracker.save()

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
        await _append_log_message(
            run, run_repo, claim_token, log_messages, assistant_message
        )
        conversation_messages = _conversation_from_log_messages(log_messages)

        # If there were parse errors, feed them back to the LLM and continue the loop
        if parse_errors:
            error_message = MessageParam(role="user", content=parse_errors)
            await _append_log_message(
                run, run_repo, claim_token, log_messages, error_message
            )
            conversation_messages = _conversation_from_log_messages(log_messages)
            continue

        # No tool calls — done
        if not tool_calls:
            logger.info(f"Agent {agent.id} run {run.id}: no tool calls, completing")
            break

        # Execute tool calls — no approval needed. Persist each result immediately.
        for tool_call in tool_calls:
            tool_name = tool_call["name"]
            logger.info(
                "Agent %s run %s: executing tool %s", agent.id, run.id, tool_name
            )

            result = await registry.execute(tool_name, tool_call["input"], context)
            tool_result = ToolResultBlockParam(
                type="tool_result",
                tool_use_id=tool_call["id"],
                content=result.content,
                is_error=result.is_error,
            )
            tool_result_message = MessageParam(role="user", content=[tool_result])
            await _append_log_message(
                run, run_repo, claim_token, log_messages, tool_result_message
            )

        conversation_messages = _conversation_from_log_messages(log_messages)

    # Generate summary using one final LLM turn
    logger.info("Agent %s run %s: generating summary", agent.id, run.id)
    summary_prompt_message = MessageParam(
        role="user",
        content=(
            "Provide a brief summary (2-3 sentences) of what you just did and the outcomes. "
            "Be factual and concise."
        ),
    )
    await _append_log_message(
        run, run_repo, claim_token, log_messages, summary_prompt_message
    )
    conversation_messages = _conversation_from_log_messages(log_messages)

    summary_blocks: list = []
    summary_tracker = UsageTracker(
        UsageRepository(),
        UsageContext(
            user_id=agent.user_id if not is_org_agent else None,
            model_id=llm_provider.model_record_id,
            model_name=llm_provider.model_name,
            provider_type=llm_provider.provider_type,
            purpose=UsagePurpose.AGENT_SUMMARY,
            agent_run_id=run.id,
        ),
    )
    raw_summary_stream = llm_provider.stream_response(
        prompt="",
        messages=conversation_messages,
        tools=[],
        max_tokens=500,
        temperature=0.3,
        system_prompt=system_prompt,
    )
    async for event in summary_tracker.wrap_stream(raw_summary_stream):
        if event.type == "content_block_start" and event.content_block.type == "text":
            summary_blocks.append(event.content_block.text)
        elif event.type == "content_block_delta" and event.delta.type == "text_delta":
            summary_blocks.append(event.delta.text)
        elif event.type == "message_stop":
            break

    summary_tracker.save()

    summary = "".join(summary_blocks).strip()

    # Memory write (fire-and-forget) — only in 'full' mode
    if (
        memory_provider is not None
        and memory_namespace is not None
        and effective_mode == MemoryMode.FULL
        and summary
    ):
        try:
            turn = [
                MessageParam(
                    role="user",
                    content=f"Agent task: {agent.instructions}",
                ),
                MessageParam(
                    role="assistant",
                    content=f"Agent run summary: {summary}",
                ),
            ]
            asyncio.create_task(
                memory_provider.add(messages=turn, key=memory_namespace)
            )
        except Exception as e:
            logger.warning(f"Memory write setup failed for agent {agent.id}: {e}")

    logger.info(f"Agent {agent.id} run {run.id} completed successfully")
    return AgentExecutionResult(summary=summary)


async def execute_claimed_agent(
    agent: Agent,
    app_state: AppState,
    run: AgentRun,
    claim_token: str,
    run_repo: AgentRunRepository,
) -> AgentExecutionResult:
    """Execute an already-claimed agent run and return its summary result."""
    return await _run_agent_loop(agent, app_state, run, run_repo, claim_token)


async def execute_agent(
    agent: Agent,
    app_state: AppState,
) -> AgentRun:
    """Synchronous helper for tests/manual callers: enqueue, claim, execute, finalize."""
    run_repo = AgentRunRepository()
    retry_policy = AgentRunRetryPolicy(
        max_attempts=AGENT_RUN_MAX_ATTEMPTS,
        backoff_delays=tuple(timedelta(seconds=s) for s in AGENT_RUN_BACKOFF_SECONDS),
    )
    created = await run_repo.create_run(
        agent.id,
        trigger_type=AgentRunTriggerType.MANUAL,
        max_attempts=AGENT_RUN_MAX_ATTEMPTS,
    )
    run = created.run if isinstance(created, AgentRunAlreadyActive) else created

    claim = await run_repo.claim_next_run(
        max_concurrent_runs=AGENT_MAX_CONCURRENT_RUNS,
        lease_duration=timedelta(seconds=AGENT_RUN_LEASE_SECONDS),
        retry_policy=retry_policy,
    )
    if claim is None or claim.run.id != run.id:
        raise RuntimeError(f"Unable to claim agent run {run.id}")

    try:
        result = await execute_claimed_agent(
            agent, app_state, claim.run, claim.claim_token, run_repo
        )
        completed = await run_repo.complete_run(
            claim.run.id, claim.claim_token, result.summary
        )
        if completed is None:
            raise RuntimeError(f"Lost claim before completing run {claim.run.id}")
        return completed
    except Exception as e:
        logger.error(
            "Agent %s run %s failed: %s", agent.id, claim.run.id, e, exc_info=True
        )
        failed = await run_repo.fail_run(
            claim.run.id, claim.claim_token, str(e), retry_policy
        )
        if failed is None:
            raise
        return failed
