"""Map paperless-ngx documents to Omni Document model and generate markdown content."""

from datetime import datetime
from typing import Any

from omni_connector import Document, DocumentMetadata, DocumentPermissions

from .config import MAX_CONTENT_LENGTH
from .models import PaperlessDocument


def generate_document_content(doc: PaperlessDocument) -> str:
    """Render a PaperlessDocument as a markdown string combining metadata and OCR content."""
    lines: list[str] = []

    lines.append(f"# {doc.title}")
    lines.append("")
    lines.append("## Metadata")

    if doc.correspondent_name:
        lines.append(f"- **Correspondent:** {doc.correspondent_name}")
    if doc.document_type_name:
        lines.append(f"- **Document Type:** {doc.document_type_name}")
    if doc.tag_names:
        lines.append(f"- **Tags:** {', '.join(sorted(doc.tag_names))}")
    if doc.created:
        lines.append(f"- **Created:** {doc.created.strftime('%Y-%m-%d')}")
    if doc.added:
        lines.append(f"- **Added:** {doc.added.strftime('%Y-%m-%d')}")
    if doc.original_file_name:
        lines.append(f"- **Original File:** {doc.original_file_name}")

    if doc.custom_fields:
        lines.append("- **Custom Fields:**")
        for cf in doc.custom_fields:
            if cf.value is not None:
                lines.append(f"  - {cf.name}: {cf.value}")

    if doc.content:
        lines.append("")
        lines.append("## Content")
        lines.append("")
        lines.append(doc.content)

    result = "\n".join(lines)
    if len(result) > MAX_CONTENT_LENGTH:
        result = result[:MAX_CONTENT_LENGTH] + "\n... (truncated)"
    return result


def map_document_to_omni(
    doc: PaperlessDocument,
    content_id: str,
    source_id: str,
    base_url: str,
) -> Document:
    """Map a PaperlessDocument to an Omni Document."""
    url = f"{base_url.rstrip('/')}/documents/{doc.id}/details/"

    attributes: dict[str, Any] = {
        "source_type": "paperless_ngx",
        "paperless_id": str(doc.id),
    }
    if doc.correspondent_name:
        attributes["correspondent"] = doc.correspondent_name
    if doc.document_type_name:
        attributes["document_type"] = doc.document_type_name
    if doc.tag_names:
        attributes["tags"] = ", ".join(sorted(doc.tag_names))
    if doc.original_file_name:
        attributes["original_file_name"] = doc.original_file_name

    return Document(
        external_id=f"paperless:{source_id}:{doc.id}",
        title=doc.title,
        content_id=content_id,
        metadata=DocumentMetadata(
            author=doc.correspondent_name,
            created_at=doc.created,
            updated_at=doc.modified,
            content_type="document",
            mime_type="text/plain",
            url=url,
        ),
        permissions=DocumentPermissions(public=True),
        attributes=attributes,
    )


def _fmt_date(dt: datetime | None) -> str:
    if dt is None:
        return ""
    return dt.strftime("%Y-%m-%d")
