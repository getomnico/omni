from datetime import UTC, datetime

SOURCE_DISPLAY_NAMES = {
    "google_drive": "Google Drive",
    "gmail": "Gmail",
    "confluence": "Confluence",
    "jira": "Jira",
    "slack": "Slack",
    "hubspot": "HubSpot",
    "fireflies": "Fireflies",
    "web": "Web",
    "local_files": "Files",
    "github": "GitHub",
    "notion": "Notion",
    "one_drive": "OneDrive",
    "share_point": "SharePoint",
    "outlook": "Outlook",
    "outlook_calendar": "Outlook Calendar",
}

SYSTEM_PROMPT_TEMPLATE = """You are Omni AI, a workplace agent that helps employees find information and complete tasks across their connected apps.

Current date and time: {current_datetime} (UTC)
{user_line}
Connected apps: {connected_apps}
{actions_section}
# Searching
- The `search_documents` tool is the primary tool to query the Omni unified index that syncs data from all of the above connected apps.
- Use inline query operators for efficient filtering: in:slack, type:pdf, status:done, by:sarah, before:2024-06, after:2024-01.
- To make an OR query, simply put both: "budget report in:slack in:gmail" - this will return results from both Slack and Gmail. multiple filters for the same operator are OR'd.
- To make an AND query, use multiple operators: "budget report in:slack type:pdf" - this will return results that are both in Slack and are PDFs. Multiple filters for different operators are AND'd.
- For time-scoped queries, use date operators or natural language: "after:2024-06 report", "budget last week", "standup yesterday".
- When asked about a person's work, use by: or from: operators: "from:sarah last week".
- Use multiple targeted searches rather than one broad search. If the first search doesn't find what you need, refine the query or try a different app.
- When results reference other documents, use `read_document` to get the full content before answering.

# Taking actions
- Before executing a write action, state exactly what you will do and why in one sentence. The user will be prompted to approve or deny.
- For read actions (data retrieval, listing), proceed without preamble.
- After an action completes, report the outcome concisely. If it failed, explain what went wrong and suggest alternatives.
- Never repeat a failed action with the same parameters. Diagnose the issue first.
- When a task requires multiple steps, execute them sequentially. Do not ask the user to confirm intermediate steps unless a decision is genuinely ambiguous.

# Sandbox (code execution)
- Use sandbox tools (`run_python`, `run_bash`, `write_file`, `read_file`) when the user needs data processing, analysis, or transformation that cannot be done with search alone.
- Use the `run_python` tool for quick one-liners; for more complex tasks, use `write_file` to create a Python script and then `run_bash` to execute it.
- To analyze a full document, use `read_document` to fetch it into the workspace, then process with `run_python` or `run_bash`. For large text documents and binary files (spreadsheets, PDFs), `read_document` automatically saves them to the workspace.
- Always print results to stdout so they appear in the output. Don't just assign to variables silently.
- If code fails, read the error, fix the issue, and retry. Don't ask the user to debug it.

# Visualization
- matplotlib and seaborn are pre-installed. Use them for charts, plots, and data visualizations.
- Always use `plt.savefig('filename.png', bbox_inches='tight')` followed by `plt.close()` to save charts as files.
- After saving a chart or generating any file the user should see, call `present_artifact(path="filename.png", title="Descriptive Title")` to display it. Without `present_artifact`, the user cannot see generated files.
- For processed spreadsheets or other output files, also use `present_artifact` so the user can download them.

# Skills
- Use `load_skill` to load detailed instructions when working with specific file types or complex tasks.
- When working with Excel/spreadsheet files, load the "excel" skill first for guidance on data boundaries, merged cells, type inference, and the `excel` CLI tool.

# Response style
- Be direct. Lead with the answer, not the process.
- Keep preambles to one short sentence at most. Don't narrate what you're about to do in detail — just do it.
- When citing information, always reference the source document.
- If you genuinely cannot find the information, say so directly rather than hedging or speculating.
- Prioritize accuracy over helpfulness. If something looks wrong, say so. Do not confirm the user's assumptions without verifying them first."""


AGENT_SYSTEM_PROMPT_TEMPLATE = """You are an automated agent running on a schedule. Your task:
{instructions}

Execute this task now using the tools available to you.
Do not ask questions — use your best judgment.
When done, provide a brief summary of what you did and the outcomes.

Current date and time: {current_datetime} (UTC)
{user_line}
Connected apps: {connected_apps}
{actions_section}
# Searching
- Use inline query operators for efficient filtering: in:slack, type:pdf, status:done, by:sarah, before:2024-06, after:2024-01.
- Use multiple targeted searches rather than one broad search.

# Taking actions
- Execute actions directly without asking for confirmation.
- After an action completes, continue with the next step.
- Never repeat a failed action with the same parameters. Diagnose the issue first.

# Response style
- Be direct and concise.
- Focus on completing the task efficiently."""


AGENT_CHAT_SYSTEM_PROMPT_TEMPLATE = """You are the "{agent_name}" agent. {user_line}is chatting with you to understand your activity and outcomes.

Your task/purpose: {agent_instructions}
Your schedule: {agent_schedule_type} — {agent_schedule_value}

{run_history_section}

Current date and time: {current_datetime} (UTC)
{user_line}
Connected apps: {connected_apps}

# Your role
- Answer questions about your previous runs, outcomes, and patterns.
- Use the run history provided above as your primary source of information. Only use tools when the user explicitly asks you to search or look something up — do not proactively make tool calls.
- Be specific: cite run dates, statuses, and summaries when answering.
- This is a read-only session. No write actions are available.

# Searching
- Use inline query operators for efficient filtering: in:slack, type:pdf, status:done, by:sarah, before:2024-06, after:2024-01.
- Use multiple targeted searches rather than one broad search.

# Response style
- Be direct. Lead with the answer.
- When citing information, reference specific runs by date."""


def _format_datetime(dt: datetime | None = None) -> str:
    if dt is None:
        dt = datetime.now(UTC)
    return dt.strftime("%A, %B %d, %Y %H:%M UTC")


def _format_user_line(
    user_name: str | None,
    user_email: str,
    prefix: str = "User",
) -> str:
    if user_name:
        identity = f"{user_name} ({user_email})"
    else:
        identity = user_email
    # Escape braces so .format() doesn't choke on user-supplied strings
    identity = identity.replace("{", "{{").replace("}", "}}")
    return f"{prefix}: {identity}"


def build_agent_system_prompt(
    agent,
    sources: list,
    connector_actions: list | None = None,
    user_name: str | None = None,
    user_email: str | None = None,
    memories: list[str] | None = None,
) -> str:
    """Build system prompt for a background agent.

    Args:
        memories: list of memory strings from previous runs to inject as agent memory
    """
    seen = set()
    display_names = []
    for source in sources:
        source_type = source.source_type
        if source_type not in seen:
            seen.add(source_type)
            name = SOURCE_DISPLAY_NAMES.get(source_type, source_type)
            display_names.append(name)

    connected_apps = ", ".join(display_names) if display_names else "None"

    actions_section = ""
    if connector_actions:
        actions_by_source: dict[str, list[str]] = {}
        for action in connector_actions:
            source_display = SOURCE_DISPLAY_NAMES.get(
                action.source_type, action.source_type
            )
            action_desc = f"  - {action.action_name}: {action.description}"
            actions_by_source.setdefault(source_display, []).append(action_desc)

        actions_lines = ["\nAvailable actions:"]
        for source_name, actions in actions_by_source.items():
            actions_lines.append(f"{source_name}:")
            actions_lines.extend(actions)

        actions_section = "\n".join(actions_lines)

    user_line = _format_user_line(user_name, user_email, prefix="Running on behalf of")

    base_prompt = AGENT_SYSTEM_PROMPT_TEMPLATE.format(
        instructions=agent.instructions,
        current_datetime=_format_datetime(),
        user_line=user_line,
        connected_apps=connected_apps,
        actions_section=actions_section,
    )

    if memories:
        bullet_list = "\n".join(f"- {m}" for m in memories)
        return base_prompt + f"\n\n## Agent memory (from prior runs)\n{bullet_list}"
    return base_prompt


def build_chat_system_prompt(
    sources: list,
    connector_actions: list | None = None,
    user_name: str | None = None,
    user_email: str | None = None,
    memories: list[str] | None = None,
) -> str:
    """Build system prompt from active sources and connector actions.

    Args:
        sources: list of Source dataclass instances (from db.models)
        connector_actions: list of ConnectorAction dataclass instances (from tools.connector_handler)
        user_name: display name of the current user
        user_email: email of the current user
        memories: list of memory strings to inject as remembered context
    """
    seen = set()
    display_names = []
    for source in sources:
        source_type = source.source_type
        if source_type not in seen:
            seen.add(source_type)
            name = SOURCE_DISPLAY_NAMES.get(source_type, source_type)
            display_names.append(name)

    connected_apps = ", ".join(display_names) if display_names else "None"

    actions_section = ""
    if connector_actions:
        actions_by_source: dict[str, list[str]] = {}
        for action in connector_actions:
            source_display = SOURCE_DISPLAY_NAMES.get(
                action.source_type, action.source_type
            )
            mode_label = (
                "read" if action.mode == "read" else "write — requires approval"
            )
            action_desc = (
                f"  - {action.action_name}: {action.description} [{mode_label}]"
            )
            actions_by_source.setdefault(source_display, []).append(action_desc)

        actions_lines = ["\nAvailable actions:"]
        for source_name, actions in actions_by_source.items():
            actions_lines.append(f"{source_name}:")
            actions_lines.extend(actions)

        actions_section = "\n".join(actions_lines)

    user_line = _format_user_line(user_name, user_email)

    base_prompt = SYSTEM_PROMPT_TEMPLATE.format(
        current_datetime=_format_datetime(),
        user_line=user_line,
        connected_apps=connected_apps,
        actions_section=actions_section,
    )

    if memories:
        bullet_list = "\n".join(f"- {m}" for m in memories)
        return base_prompt + f"\n\n## Remembered context about this user\n{bullet_list}"
    return base_prompt


def _format_execution_log(execution_log: list[dict], max_chars: int = 5000) -> str:
    """Format an agent run's execution log into a condensed summary of tool calls."""
    if not execution_log:
        return "  (no execution log)"

    lines = []
    total_chars = 0

    for msg in execution_log:
        role = msg.get("role", "")
        content = msg.get("content", "")

        if role == "assistant" and isinstance(content, list):
            for block in content:
                if block.get("type") == "tool_use":
                    tool_line = f"  Tool call: {block.get('name', '?')}"
                    tool_input = block.get("input", {})
                    if isinstance(tool_input, dict):
                        # Show key params concisely
                        params = ", ".join(
                            f"{k}={repr(v)[:100]}" for k, v in tool_input.items()
                        )
                        if params:
                            tool_line += f"({params})"
                    lines.append(tool_line)
                elif block.get("type") == "text" and block.get("text"):
                    text = block["text"][:500]
                    lines.append(f"  Agent said: {text}")

        elif role == "user" and isinstance(content, list):
            for block in content:
                if block.get("type") == "tool_result":
                    result_content = block.get("content", "")
                    is_error = block.get("is_error", False)
                    prefix = "  Tool error:" if is_error else "  Tool result:"
                    if isinstance(result_content, list):
                        # Extract text from content blocks
                        texts = [
                            b.get("text", "")[:300]
                            for b in result_content
                            if isinstance(b, dict) and b.get("type") == "text"
                        ]
                        if texts:
                            lines.append(f"{prefix} {'; '.join(texts)}")
                        else:
                            # Count search results etc.
                            search_count = sum(
                                1
                                for b in result_content
                                if isinstance(b, dict)
                                and b.get("type") == "search_result"
                            )
                            if search_count:
                                lines.append(f"{prefix} {search_count} search results")
                    elif isinstance(result_content, str):
                        lines.append(f"{prefix} {result_content[:300]}")

        total_chars += sum(len(l) for l in lines) - total_chars
        if total_chars > max_chars:
            lines.append("  ... (log truncated)")
            break

    return "\n".join(lines) if lines else "  (no tool activity)"


def format_run_history(runs: list, max_detailed: int = 3) -> str:
    """Format agent run history for injection into the system prompt.

    Args:
        runs: list of AgentRun objects, ordered most recent first.
        max_detailed: number of most recent runs to include detailed execution logs for.

    Returns:
        Formatted string summarizing the run history.
    """
    if not runs:
        return "No runs recorded yet."

    max_total_chars = 30000
    sections = []
    total_chars = 0

    sections.append(f"## Agent Run History ({len(runs)} most recent runs)\n")

    for i, run in enumerate(runs):
        started = (
            run.started_at.strftime("%Y-%m-%d %H:%M UTC") if run.started_at else "N/A"
        )
        completed = (
            run.completed_at.strftime("%Y-%m-%d %H:%M UTC")
            if run.completed_at
            else "N/A"
        )

        header = f"### Run {i+1} — {started}"
        header += f"\n- Status: {run.status}"
        header += f"\n- Completed: {completed}"

        if run.summary:
            header += f"\n- Summary: {run.summary}"
        if run.error_message:
            header += f"\n- Error: {run.error_message}"

        if i < max_detailed and run.execution_log:
            header += "\n- Execution details:\n"
            header += _format_execution_log(run.execution_log)

        sections.append(header)
        total_chars += len(header)

        if total_chars > max_total_chars:
            sections.append(f"\n... ({len(runs) - i - 1} older runs omitted)")
            break

    return "\n\n".join(sections)


def build_agent_chat_system_prompt(
    agent,
    runs: list,
    sources: list,
    user_name: str | None = None,
    user_email: str | None = None,
) -> str:
    """Build system prompt for an interactive chat session with an agent."""
    seen = set()
    display_names = []
    for source in sources:
        source_type = source.source_type
        if source_type not in seen:
            seen.add(source_type)
            name = SOURCE_DISPLAY_NAMES.get(source_type, source_type)
            display_names.append(name)

    connected_apps = ", ".join(display_names) if display_names else "None"
    user_line = _format_user_line(user_name, user_email)
    run_history_section = format_run_history(runs)

    return AGENT_CHAT_SYSTEM_PROMPT_TEMPLATE.format(
        agent_name=agent.name,
        agent_instructions=agent.instructions,
        agent_schedule_type=agent.schedule_type,
        agent_schedule_value=agent.schedule_value,
        run_history_section=run_history_section,
        current_datetime=_format_datetime(),
        user_line=user_line,
        connected_apps=connected_apps,
    )
