import json
from collections.abc import Mapping


def extract_text_document(block: Mapping[str, object]) -> str | None:
    """Convert a text-backed document block into provider-neutral text."""
    if block.get("type") != "document":
        return None

    source = block.get("source")
    if not isinstance(source, Mapping) or source.get("type") != "text":
        return None

    data = source.get("data")
    if not isinstance(data, str) or not data:
        return None

    title = block.get("title")
    if isinstance(title, str) and title:
        return f"Document title: {json.dumps(title, ensure_ascii=False)}\nDocument content:\n{data}"
    return f"Document content:\n{data}"
