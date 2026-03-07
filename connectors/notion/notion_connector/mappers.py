"""Notion API responses to Omni Document mapping and content rendering."""

from datetime import datetime
from typing import Any

from omni_connector import Document, DocumentMetadata, DocumentPermissions

from .config import MAX_CONTENT_LENGTH


def map_page_to_document(
    page: dict[str, Any],
    content_id: str,
    is_database_entry: bool = False,
) -> Document:
    """Map a Notion page to an Omni Document."""
    page_id = page["id"]
    title = _extract_page_title(page)
    created_by = page.get("created_by", {})
    parent = page.get("parent", {})

    content_type_value = "database_entry" if is_database_entry else "page"

    attributes: dict[str, Any] = {
        "source_type": "notion",
    }
    if is_database_entry and parent.get("type") == "database_id":
        attributes["parent_database"] = parent["database_id"]

    return Document(
        external_id=f"notion:page:{page_id}",
        title=title,
        content_id=content_id,
        metadata=DocumentMetadata(
            author=created_by.get("id"),
            created_at=_parse_iso(page.get("created_time")),
            updated_at=_parse_iso(page.get("last_edited_time")),
            url=page.get("url"),
            content_type=content_type_value,
            mime_type="text/plain",
        ),
        permissions=DocumentPermissions(public=False),
        attributes=attributes,
    )


def map_database_to_document(
    database: dict[str, Any],
    content_id: str,
) -> Document:
    """Map a Notion database to an Omni Document."""
    db_id = database["id"]
    title = _extract_rich_text(database.get("title", []))
    created_by = database.get("created_by", {})

    return Document(
        external_id=f"notion:database:{db_id}",
        title=title or "Untitled Database",
        content_id=content_id,
        metadata=DocumentMetadata(
            author=created_by.get("id"),
            created_at=_parse_iso(database.get("created_time")),
            updated_at=_parse_iso(database.get("last_edited_time")),
            url=database.get("url"),
            content_type="database",
            mime_type="text/plain",
        ),
        permissions=DocumentPermissions(public=False),
        attributes={
            "source_type": "notion",
        },
    )


def generate_page_content(
    page: dict[str, Any],
    blocks: list[dict[str, Any]],
    properties: dict[str, Any] | None = None,
) -> str:
    """Compose full searchable text for a page."""
    lines: list[str] = []
    title = _extract_page_title(page)
    lines.append(title)
    lines.append("")

    if properties:
        prop_text = render_page_properties(properties)
        if prop_text:
            lines.append(prop_text)
            lines.append("")

    block_text = render_blocks_to_text(blocks)
    if block_text:
        lines.append(block_text)

    return _truncate("\n".join(lines))


def generate_database_content(database: dict[str, Any]) -> str:
    """Generate searchable text for a database."""
    lines: list[str] = []
    title = _extract_rich_text(database.get("title", []))
    lines.append(f"Database: {title or 'Untitled'}")

    description = _extract_rich_text(database.get("description", []))
    if description:
        lines.append(f"Description: {description}")

    props = database.get("properties", {})
    if props:
        lines.append("")
        lines.append("Properties:")
        for name, prop in props.items():
            prop_type = prop.get("type", "unknown")
            lines.append(f"  - {name} ({prop_type})")

    return _truncate("\n".join(lines))


def render_blocks_to_text(blocks: list[dict[str, Any]], depth: int = 0) -> str:
    """Convert Notion block tree to plain text."""
    lines: list[str] = []
    indent = "  " * depth

    for block in blocks:
        block_type = block.get("type", "")
        block_data = block.get(block_type, {})

        text = ""
        if block_type in ("paragraph", "quote", "callout"):
            text = _extract_rich_text(block_data.get("rich_text", []))
        elif block_type == "heading_1":
            text = _extract_rich_text(block_data.get("rich_text", []))
            if text:
                text = f"# {text}"
        elif block_type == "heading_2":
            text = _extract_rich_text(block_data.get("rich_text", []))
            if text:
                text = f"## {text}"
        elif block_type == "heading_3":
            text = _extract_rich_text(block_data.get("rich_text", []))
            if text:
                text = f"### {text}"
        elif block_type == "bulleted_list_item":
            text = _extract_rich_text(block_data.get("rich_text", []))
            if text:
                text = f"- {text}"
        elif block_type == "numbered_list_item":
            text = _extract_rich_text(block_data.get("rich_text", []))
            if text:
                text = f"1. {text}"
        elif block_type == "to_do":
            text = _extract_rich_text(block_data.get("rich_text", []))
            checked = block_data.get("checked", False)
            marker = "[x]" if checked else "[ ]"
            if text:
                text = f"{marker} {text}"
        elif block_type == "code":
            text = _extract_rich_text(block_data.get("rich_text", []))
            lang = block_data.get("language", "")
            if text:
                text = f"```{lang}\n{text}\n```"
        elif block_type == "toggle":
            text = _extract_rich_text(block_data.get("rich_text", []))
        elif block_type == "divider":
            text = "---"
        elif block_type == "table":
            text = _render_table(block)
        elif block_type == "table_row":
            cells = block_data.get("cells", [])
            cell_texts = [_extract_rich_text(cell) for cell in cells]
            text = " | ".join(cell_texts)
        elif block_type == "child_page":
            title = block_data.get("title", "")
            if title:
                text = f"[Page: {title}]"
        elif block_type == "child_database":
            title = block_data.get("title", "")
            if title:
                text = f"[Database: {title}]"
        elif block_type == "bookmark":
            url = block_data.get("url", "")
            caption = _extract_rich_text(block_data.get("caption", []))
            text = caption or url
        elif block_type == "equation":
            text = block_data.get("expression", "")

        if text:
            lines.append(f"{indent}{text}")

        children = block.get("_children", [])
        if children:
            child_text = render_blocks_to_text(children, depth + 1)
            if child_text:
                lines.append(child_text)

    return "\n".join(lines)


def _render_table(block: dict[str, Any]) -> str:
    """Render a table block by processing its child rows."""
    children = block.get("_children", [])
    if not children:
        return ""
    rows: list[str] = []
    for row in children:
        if row.get("type") == "table_row":
            cells = row.get("table_row", {}).get("cells", [])
            cell_texts = [_extract_rich_text(cell) for cell in cells]
            rows.append(" | ".join(cell_texts))
    return "\n".join(rows)


def render_page_properties(properties: dict[str, Any]) -> str:
    """Convert database entry properties to text."""
    lines: list[str] = []
    for name, prop in properties.items():
        value = _extract_property_value(prop)
        if value:
            lines.append(f"{name}: {value}")
    return "\n".join(lines)


def _extract_property_value(prop: dict[str, Any]) -> str:
    """Extract a displayable string from a Notion property value."""
    prop_type = prop.get("type", "")

    if prop_type == "title":
        return _extract_rich_text(prop.get("title", []))
    elif prop_type == "rich_text":
        return _extract_rich_text(prop.get("rich_text", []))
    elif prop_type == "number":
        val = prop.get("number")
        return str(val) if val is not None else ""
    elif prop_type == "select":
        sel = prop.get("select")
        return sel["name"] if sel else ""
    elif prop_type == "multi_select":
        items = prop.get("multi_select", [])
        return ", ".join(item["name"] for item in items)
    elif prop_type == "date":
        date = prop.get("date")
        if not date:
            return ""
        start = date.get("start", "")
        end = date.get("end")
        return f"{start} → {end}" if end else start
    elif prop_type == "checkbox":
        return "Yes" if prop.get("checkbox") else "No"
    elif prop_type == "url":
        return prop.get("url", "") or ""
    elif prop_type == "email":
        return prop.get("email", "") or ""
    elif prop_type == "phone_number":
        return prop.get("phone_number", "") or ""
    elif prop_type == "people":
        people = prop.get("people", [])
        return ", ".join(p.get("name", p.get("id", "")) for p in people)
    elif prop_type == "relation":
        relations = prop.get("relation", [])
        return ", ".join(r.get("id", "") for r in relations)
    elif prop_type == "formula":
        formula = prop.get("formula", {})
        f_type = formula.get("type", "")
        return str(formula.get(f_type, "")) if f_type else ""
    elif prop_type == "status":
        status = prop.get("status")
        return status["name"] if status else ""
    elif prop_type == "rollup":
        rollup = prop.get("rollup", {})
        r_type = rollup.get("type", "")
        if r_type == "number":
            val = rollup.get("number")
            return str(val) if val is not None else ""
        elif r_type == "array":
            items = rollup.get("array", [])
            return ", ".join(_extract_property_value(item) for item in items if item)
        return ""
    elif prop_type == "created_time":
        return prop.get("created_time", "")
    elif prop_type == "last_edited_time":
        return prop.get("last_edited_time", "")
    elif prop_type == "created_by":
        user = prop.get("created_by", {})
        return user.get("name", user.get("id", ""))
    elif prop_type == "last_edited_by":
        user = prop.get("last_edited_by", {})
        return user.get("name", user.get("id", ""))

    return ""


def _extract_page_title(page: dict[str, Any]) -> str:
    """Extract the title from a Notion page."""
    properties = page.get("properties", {})
    for prop in properties.values():
        if prop.get("type") == "title":
            return _extract_rich_text(prop.get("title", []))
    return "Untitled"


def _extract_rich_text(rich_text: list[dict[str, Any]]) -> str:
    """Extract plain text from a Notion rich_text array."""
    return "".join(item.get("plain_text", "") for item in rich_text)


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
