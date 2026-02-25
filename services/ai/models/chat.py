from typing import Any

from pydantic import BaseModel


class SearchToolParams(BaseModel):
    query: str
    document_id: str | None = None
    sources: list[str] | None = None
    content_types: list[str] | None = None
    attributes: dict[str, Any] | None = None
    limit: int | None = 20
