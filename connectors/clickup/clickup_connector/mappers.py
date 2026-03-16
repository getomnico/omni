"""Map ClickUp API objects to Omni Document model."""

from datetime import datetime, timezone
from typing import Any

from omni_connector import Document, DocumentMetadata, DocumentPermissions

from .config import MAX_CONTENT_LENGTH


# ── Hierarchy lookup ────────────────────────────────────────────────


class HierarchyLookup:
    """Resolves list_id to space/folder/list names for document path and attributes."""

    def __init__(self) -> None:
        self._lists: dict[str, dict[str, str]] = {}

    def register_list(
        self, list_id: str, list_name: str, space_name: str, folder_name: str = ""
    ) -> None:
        self._lists[list_id] = {
            "list_name": list_name,
            "space_name": space_name,
            "folder_name": folder_name,
        }

    def get(self, list_id: str) -> dict[str, str]:
        return self._lists.get(
            list_id,
            {
                "list_name": "",
                "space_name": "",
                "folder_name": "",
            },
        )


# ── Task → Document ────────────────────────────────────────────────


def map_task_to_document(
    task: dict[str, Any],
    comments: list[dict[str, Any]],
    content_id: str,
    team_id: str,
    hierarchy: HierarchyLookup,
) -> Document:
    list_info = hierarchy.get(task.get("list", {}).get("id", ""))
    space_name = list_info["space_name"]
    list_name = list_info["list_name"]
    folder_name = list_info["folder_name"]

    title_prefix = f"{space_name} / {list_name}" if space_name else list_name
    task_name = task.get("name", "Untitled")

    assignees = task.get("assignees", [])
    assignee_names = ",".join(
        a.get("username", "") for a in assignees if a.get("username")
    )
    assignee_emails = ",".join(a.get("email", "") for a in assignees if a.get("email"))

    tags = task.get("tags", [])
    tag_names = ",".join(t.get("name", "") for t in tags if t.get("name"))

    priority = task.get("priority")
    priority_name = priority.get("priority", "") if isinstance(priority, dict) else ""

    status = task.get("status", {})
    status_name = status.get("status", "") if isinstance(status, dict) else ""

    creator = task.get("creator", {})
    creator_name = creator.get("username") if isinstance(creator, dict) else None

    path_parts = [space_name, folder_name, list_name]
    path = " / ".join(p for p in path_parts if p)

    return Document(
        external_id=f"clickup:task:{task['id']}",
        title=f"[{title_prefix}] {task_name}" if title_prefix else task_name,
        content_id=content_id,
        metadata=DocumentMetadata(
            author=creator_name,
            created_at=_from_unix_ms(task.get("date_created")),
            updated_at=_from_unix_ms(task.get("date_updated")),
            content_type="task",
            mime_type="text/plain",
            url=task.get("url"),
            path=path,
        ),
        permissions=DocumentPermissions(
            public=False,
            groups=[f"clickup:workspace:{team_id}"],
        ),
        attributes={
            "source_type": "clickup",
            "status": status_name,
            "priority": priority_name,
            "assignee": assignee_names,
            "assignee_email": assignee_emails,
            "tags": tag_names,
            "list_name": list_name,
            "space_name": space_name,
            "folder_name": folder_name,
            "task_type": "subtask" if task.get("parent") else "task",
            "due_date": _format_due_date(task.get("due_date")),
        },
    )


def generate_task_content(
    task: dict[str, Any],
    comments: list[dict[str, Any]],
    hierarchy: HierarchyLookup,
) -> str:
    list_info = hierarchy.get(task.get("list", {}).get("id", ""))

    lines: list[str] = []
    lines.append(f"Task: {task.get('name', 'Untitled')}")

    status = task.get("status", {})
    if isinstance(status, dict) and status.get("status"):
        lines.append(f"Status: {status['status']}")

    priority = task.get("priority")
    if isinstance(priority, dict) and priority.get("priority"):
        lines.append(f"Priority: {priority['priority']}")

    assignees = task.get("assignees", [])
    if assignees:
        names = ", ".join(a.get("username", a.get("email", "")) for a in assignees)
        lines.append(f"Assignee: {names}")

    path_parts = [
        list_info["space_name"],
        list_info["folder_name"],
        list_info["list_name"],
    ]
    path = " > ".join(p for p in path_parts if p)
    if path:
        lines.append(f"Space: {path}")

    due_date = _format_due_date(task.get("due_date"))
    if due_date:
        lines.append(f"Due: {due_date}")

    lines.append("")

    description = task.get("description") or task.get("text_content") or ""
    if description:
        lines.append(description)

    # Custom fields
    custom_fields = task.get("custom_fields", [])
    cf_lines: list[str] = []
    for cf in custom_fields:
        name = cf.get("name", "")
        value = cf.get("value")
        if name and value is not None and value != "":
            cf_lines.append(f"{name}: {value}")
    if cf_lines:
        lines.append("")
        lines.append("--- Custom Fields ---")
        lines.extend(cf_lines)

    # Comments
    if comments:
        lines.append("")
        lines.append("--- Comments ---")
        for c in comments:
            user = c.get("user", {})
            author = user.get("username", user.get("email", "unknown"))
            text = c.get("comment_text", "")
            lines.append(f"\n{author}:")
            if text:
                lines.append(text)

    return _truncate("\n".join(lines))


# ── Doc → Document ──────────────────────────────────────────────────


def map_doc_to_document(
    doc: dict[str, Any],
    pages_content: str,
    content_id: str,
    team_id: str,
) -> Document:
    return Document(
        external_id=f"clickup:doc:{doc['id']}",
        title=doc.get("name", "Untitled Doc"),
        content_id=content_id,
        metadata=DocumentMetadata(
            author=None,
            created_at=_from_unix_ms(doc.get("date_created")),
            updated_at=_from_unix_ms(doc.get("date_updated")),
            content_type="doc",
            mime_type="text/plain",
            url=None,
        ),
        permissions=DocumentPermissions(
            public=False,
            groups=[f"clickup:workspace:{team_id}"],
        ),
        attributes={
            "source_type": "clickup",
        },
    )


def generate_doc_content(doc: dict[str, Any], pages: list[dict[str, Any]]) -> str:
    lines: list[str] = []
    lines.append(f"Doc: {doc.get('name', 'Untitled')}")
    lines.append("")

    for page in pages:
        page_name = page.get("name", "Untitled Page")
        lines.append(f"[Page: {page_name}]")
        content = page.get("content", "")
        if content:
            lines.append(content)
        lines.append("")

    return _truncate("\n".join(lines))


# ── Helpers ─────────────────────────────────────────────────────────


def _from_unix_ms(value: Any) -> datetime | None:
    if value is None:
        return None
    try:
        ts = int(value) / 1000.0
        return datetime.fromtimestamp(ts, tz=timezone.utc)
    except (ValueError, TypeError, OSError):
        return None


def _format_due_date(value: Any) -> str:
    dt = _from_unix_ms(value)
    if dt is None:
        return ""
    return dt.strftime("%Y-%m-%d")


def _truncate(content: str) -> str:
    if len(content) > MAX_CONTENT_LENGTH:
        return content[:MAX_CONTENT_LENGTH] + "\n... (truncated)"
    return content
