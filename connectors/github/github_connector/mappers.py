"""GitHub objects to Omni Document mapping functions."""

from datetime import datetime
from typing import Any

from omni_connector import Document, DocumentMetadata, DocumentPermissions

from .config import MAX_CONTENT_LENGTH


def map_repo_to_document(
    repo: Any,
    readme_content: str | None,
    content_id: str,
) -> Document:
    """Map a GitHub repository to an Omni Document."""
    full_name = repo.full_name
    is_private = repo.private

    topics = []
    if repo.topics:
        topics = list(repo.topics)

    return Document(
        external_id=f"github:repo:{full_name}",
        title=full_name,
        content_id=content_id,
        metadata=DocumentMetadata(
            author=repo.owner.login if repo.owner else None,
            created_at=_to_datetime(repo.created_at),
            updated_at=_to_datetime(repo.updated_at),
            url=repo.html_url,
            mime_type="text/plain",
        ),
        permissions=_build_permissions(is_private, full_name),
        attributes={
            "source_type": "github",
            "content_type": "repository",
            "language": repo.language or "",
            "visibility": "private" if is_private else "public",
            "archived": (
                str(repo.archived).lower() if repo.archived is not None else "false"
            ),
            "topics": ",".join(topics) if topics else "",
        },
    )


def map_issue_to_document(
    issue: Any,
    comments: list[Any],
    content_id: str,
    repo_full_name: str,
    is_private: bool,
) -> Document:
    """Map a GitHub issue to an Omni Document."""
    labels = [l.name for l in (issue.labels or []) if hasattr(l, "name")]
    assignee_login = issue.assignee.login if issue.assignee else ""
    milestone_title = issue.milestone.title if issue.milestone else ""

    return Document(
        external_id=f"github:issue:{repo_full_name}#{issue.number}",
        title=f"[{repo_full_name}] Issue #{issue.number}: {issue.title}",
        content_id=content_id,
        metadata=DocumentMetadata(
            author=issue.user.login if issue.user else None,
            created_at=_to_datetime(issue.created_at),
            updated_at=_to_datetime(issue.updated_at),
            url=issue.html_url,
            mime_type="text/plain",
        ),
        permissions=_build_permissions(is_private, repo_full_name),
        attributes={
            "source_type": "github",
            "content_type": "issue",
            "state": issue.state or "",
            "labels": ",".join(labels),
            "assignee": assignee_login,
            "milestone": milestone_title,
        },
    )


def map_pr_to_document(
    pr: Any,
    issue_comments: list[Any],
    review_comments: list[Any],
    content_id: str,
    repo_full_name: str,
    is_private: bool,
) -> Document:
    """Map a GitHub pull request to an Omni Document."""
    labels = [l.name for l in (pr.labels or []) if hasattr(l, "name")]
    is_merged = bool(pr.merged_at)
    is_draft = bool(pr.draft) if hasattr(pr, "draft") else False

    return Document(
        external_id=f"github:pr:{repo_full_name}#{pr.number}",
        title=f"[{repo_full_name}] PR #{pr.number}: {pr.title}",
        content_id=content_id,
        metadata=DocumentMetadata(
            author=pr.user.login if pr.user else None,
            created_at=_to_datetime(pr.created_at),
            updated_at=_to_datetime(pr.updated_at),
            url=pr.html_url,
            mime_type="text/plain",
        ),
        permissions=_build_permissions(is_private, repo_full_name),
        attributes={
            "source_type": "github",
            "content_type": "pull_request",
            "state": pr.state or "",
            "draft": str(is_draft).lower(),
            "labels": ",".join(labels),
            "merged": str(is_merged).lower(),
        },
    )


def map_discussion_to_document(
    discussion: dict[str, Any],
    content_id: str,
    repo_full_name: str,
    is_private: bool,
) -> Document:
    """Map a GitHub discussion (from GraphQL) to an Omni Document."""
    number = discussion.get("number", 0)
    title = discussion.get("title", "Untitled Discussion")
    author = discussion.get("author", {}) or {}
    category = discussion.get("category", {}) or {}
    is_answered = discussion.get("answerChosenAt") is not None

    return Document(
        external_id=f"github:discussion:{repo_full_name}#{number}",
        title=f"[{repo_full_name}] Discussion #{number}: {title}",
        content_id=content_id,
        metadata=DocumentMetadata(
            author=author.get("login"),
            created_at=_parse_iso(discussion.get("createdAt")),
            updated_at=_parse_iso(discussion.get("updatedAt")),
            url=discussion.get("url"),
            mime_type="text/plain",
        ),
        permissions=_build_permissions(is_private, repo_full_name),
        attributes={
            "source_type": "github",
            "content_type": "discussion",
            "category": category.get("name", ""),
            "answered": str(is_answered).lower(),
        },
    )


def generate_repo_content(repo: Any, readme_content: str | None) -> str:
    """Generate searchable text content from a repository."""
    lines: list[str] = []
    lines.append(f"Repository: {repo.full_name}")
    if repo.description:
        lines.append(f"Description: {repo.description}")
    if repo.language:
        lines.append(f"Language: {repo.language}")
    if repo.topics:
        lines.append(f"Topics: {', '.join(repo.topics)}")
    lines.append("")
    if readme_content:
        lines.append("README:")
        lines.append(readme_content)
    return _truncate("\n".join(lines))


def generate_issue_content(issue: Any, comments: list[Any]) -> str:
    """Generate searchable text content from an issue and its comments."""
    lines: list[str] = []
    lines.append(f"Issue #{issue.number}: {issue.title}")
    lines.append(f"State: {issue.state}")
    if issue.user:
        lines.append(f"Author: {issue.user.login}")
    lines.append("")
    if issue.body:
        lines.append(issue.body)
    if comments:
        lines.append("")
        lines.append("--- Comments ---")
        for c in comments:
            author = c.user.login if c.user else "unknown"
            lines.append(f"\n{author}:")
            if c.body:
                lines.append(c.body)
    return _truncate("\n".join(lines))


def generate_pr_content(
    pr: Any, issue_comments: list[Any], review_comments: list[Any]
) -> str:
    """Generate searchable text content from a pull request and its comments."""
    lines: list[str] = []
    lines.append(f"Pull Request #{pr.number}: {pr.title}")
    lines.append(f"State: {pr.state}")
    if pr.user:
        lines.append(f"Author: {pr.user.login}")
    if pr.merged_at:
        lines.append("Merged: yes")
    lines.append("")
    if pr.body:
        lines.append(pr.body)
    if issue_comments:
        lines.append("")
        lines.append("--- Comments ---")
        for c in issue_comments:
            author = c.user.login if c.user else "unknown"
            lines.append(f"\n{author}:")
            if c.body:
                lines.append(c.body)
    if review_comments:
        lines.append("")
        lines.append("--- Review Comments ---")
        for c in review_comments:
            author = c.user.login if c.user else "unknown"
            path = c.path if hasattr(c, "path") else ""
            lines.append(f"\n{author} on {path}:")
            if c.body:
                lines.append(c.body)
    return _truncate("\n".join(lines))


def generate_discussion_content(discussion: dict[str, Any]) -> str:
    """Generate searchable text content from a discussion."""
    lines: list[str] = []
    lines.append(
        f"Discussion #{discussion.get('number', 0)}: {discussion.get('title', '')}"
    )
    author = discussion.get("author", {}) or {}
    if author.get("login"):
        lines.append(f"Author: {author['login']}")
    category = discussion.get("category", {}) or {}
    if category.get("name"):
        lines.append(f"Category: {category['name']}")
    lines.append("")
    body = discussion.get("body", "")
    if body:
        lines.append(body)
    comments_data = discussion.get("comments", {}) or {}
    comment_nodes = comments_data.get("nodes", [])
    if comment_nodes:
        lines.append("")
        lines.append("--- Comments ---")
        for c in comment_nodes:
            c_author = (c.get("author") or {}).get("login", "unknown")
            lines.append(f"\n{c_author}:")
            if c.get("body"):
                lines.append(c["body"])
    return _truncate("\n".join(lines))


def _build_permissions(is_private: bool, repo_full_name: str) -> DocumentPermissions:
    if is_private:
        return DocumentPermissions(
            public=False, groups=[f"github:repo:{repo_full_name}"]
        )
    return DocumentPermissions(public=True)


def _to_datetime(value: Any) -> datetime | None:
    if value is None:
        return None
    if isinstance(value, datetime):
        return value
    if isinstance(value, str):
        return _parse_iso(value)
    return None


def _parse_iso(value: str | None) -> datetime | None:
    if not value:
        return None
    try:
        return datetime.fromisoformat(value.replace("Z", "+00:00"))
    except (ValueError, TypeError):
        return None


def _truncate(content: str) -> str:
    if len(content) > MAX_CONTENT_LENGTH:
        return content[:MAX_CONTENT_LENGTH] + "\n... (truncated)"
    return content
