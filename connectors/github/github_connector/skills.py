"""Connector-owned skills guiding common GitHub workflows through MCP tools.

Tool names reference github-mcp-server v1.9.0 with the connector's curated
toolset list (see config.MCP_TOOLSETS).
"""

from __future__ import annotations

from typing import TYPE_CHECKING

from .config import MCP_TOOLSETS

if TYPE_CHECKING:
    from omni_connector import ConnectorSkillDefinition

_TOOLS_NOTE = (
    f"Tools are provided by the GitHub MCP server with toolsets enabled: "
    f"{', '.join(MCP_TOOLSETS)}. Every GitHub tool needs `owner` and `repo` "
    "unless stated otherwise. Use values from search results or the indexed "
    "documents instead of guessing."
)

_INVESTIGATE_ISSUE = f"""Investigate a GitHub issue end to end.

{_TOOLS_NOTE}

Workflow:

1. Locate the issue: prefer the Omni index (issue documents carry
   `github_issue_number`, `repo`, and `url` attributes). Otherwise use
   `list_issues` or `search_issues` with the repo and a natural-language
   query.
2. Read the issue with `issue_read` using `method: "get"`, then pull the
   discussion with `method: "get_comments"` and its labels with
   `method: "get_labels"`. Use `method: "get_sub_issues"` or
   `method: "get_parent"` when hierarchy matters.
3. Add repo context when needed: `get_file_contents` or
   `get_repository_tree` for the affected paths, and `actions_list` /
   `actions_get` / `get_job_logs` if CI is implicated.
4. Summarize: what the issue is, who reported it, current state, labels,
   key comments with authors, likely root cause, and suggested next step.
   Cite issue numbers.
"""

_SUMMARIZE_PR = f"""Summarize a GitHub pull request.

{_TOOLS_NOTE}

Workflow:

1. Find the PR with `list_pull_requests` (filter by state/base/head) or
   `search_pull_requests` with a GitHub search query.
2. Read it with `pull_request_read`: `method: "get"` for metadata,
   `method: "get_files"` for changed files, `method: "get_diff"` for the
   patch, `method: "get_comments"` and `method: "get_review_comments"`
   for discussion, `method: "get_status"` for CI checks, and
   `method: "get_reviews"` for review verdicts.
3. Summarize: purpose, scope of changes (files/areas), review and CI
   status, open threads that block merge, and a recommendation. Cite the
   PR number.
"""

_COMMENT_ISSUE_OR_PR = f"""Comment on a GitHub issue or pull request as the connected user.

{_TOOLS_NOTE}

Workflow:

1. Resolve the exact issue/PR number and repo first (index attributes or
   `list_issues` / `pull_request_read`). Never guess numbers.
2. Draft the comment: concise, factual, grounded in what you read. Match
   the tone of the thread.
3. Post with `add_issue_comment` (works for both issues and PRs). For PR
   review replies use `add_reply_to_pull_request_comment` with the numeric
   comment ID from a #discussion_r anchor.
4. `add_issue_comment` is a write action: it requires explicit user
   approval in chat. Always show the user the exact body before calling.
"""

_CREATE_OR_UPDATE_ISSUE = f"""Create or update GitHub issues as the connected user.

{_TOOLS_NOTE}

Creating an issue:

1. Identify the repo (`owner`/`repo`) from the Omni index or `get_me` +
   `list_issues`. Confirm with the user if ambiguous.
2. Check for duplicates first with `search_issues` (natural-language
   query). If a duplicate exists, ask the user whether to comment there
   instead.
3. Draft title (imperative, <= 70 chars) and body (context, steps to
   reproduce, expected vs actual). Ask the user to confirm before
   creating.
4. Create with `issue_write` using `method: "create"`. Set `labels` only
   if they exist (`list_label` to verify); set `assignees` only on
   explicit request.

Updating an issue:

- Read the issue first with `issue_read` (`method: "get"`,
  `method: "get_labels"`).
- Update with `issue_write` using `method: "update"` and the existing
  `issue_number`; only send fields that change. Prefer commenting over
  editing someone else's description.
- Both paths are write actions requiring chat approval.
"""


def connector_skills() -> list[ConnectorSkillDefinition]:
    from omni_connector import ConnectorSkillDefinition

    defs = [
        ("github-investigate-issue", "Investigate a GitHub issue", _INVESTIGATE_ISSUE),
        ("github-summarize-pr", "Summarize a pull request", _SUMMARIZE_PR),
        ("github-comment", "Comment on an issue or PR", _COMMENT_ISSUE_OR_PR),
        (
            "github-manage-issues",
            "Create or update issues",
            _CREATE_OR_UPDATE_ISSUE,
        ),
    ]
    return [
        ConnectorSkillDefinition(
            id=skill_id,
            title=title,
            description=title,
            source_types=["github"],
            content=content,
        )
        for skill_id, title, content in defs
    ]
